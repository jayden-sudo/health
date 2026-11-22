//! Backup interop (DB design §5.3), ported from the web `db/merge.ts`.
//! A plain (unencrypted) copy of the database travels inside the pack. Merging follows the sync rules, never "last file wins":
//!   metrics by code, sources by (connector, external_id), imports by uid (done / erased / review);
//!   observations: larger import.uid wins; corrections: CORRECTION_WINS; remote tombstones erase local imports;
//!   a locally erased import is never revived. Local integer ids are always translated, never copied.
//! ATTACH / DETACH cannot run in a transaction, so these functions must be called outside one.

use crate::correction::CORRECTION_WINS;
use crate::daily::Scope;
use crate::db::{Db, one, schema_body, tx};
use crate::error::{Error, Result};
use crate::write::{Span, parse_meta, set_span, span_of};
use serde::Serialize;
use std::collections::{BTreeSet, HashMap};
use std::path::Path;

const TABLES: &[&str] = &["metric", "metric_alias", "source", "import", "observation", "correction", "genotype"];

#[derive(Debug, Default, Serialize, schemars::JsonSchema)]
pub struct MergeReport {
    pub added_metrics: u64,
    pub added_sources: u64,
    pub added_imports: u64,
    /// Local imports erased by the other side's tombstones
    pub erased_imports: u64,
    /// Metrics whose unit or agg differs from the local one: none of their rows were merged
    pub skipped_metrics: Vec<String>,
    /// New imports whose blob another local import already holds (same file imported on two devices)
    pub dropped_blobs: u64,
    /// Blobs the new imports reference; the caller restores those it has
    pub needed_blobs: Vec<String>,
    /// Blobs no longer referenced after erasing; delete after commit
    pub orphan_blobs: Vec<String>,
    /// New imports refused: a uid from the future, or a non-UUIDv7 uid outside lab/staff/room results
    pub refused_imports: u64,
    /// Corrections refused: an HLC from the future
    pub refused_corrections: u64,
}

/// Schema statements with every object created in schema `plain`.
fn plain_schema() -> String {
    let mut out = String::new();
    for line in schema_body().lines() {
        let mut l = line.to_string();
        for kind in ["CREATE UNIQUE INDEX ", "CREATE INDEX ", "CREATE TABLE ", "CREATE VIEW ", "CREATE VIRTUAL TABLE "] {
            if let Some(rest) = line.strip_prefix(kind) {
                l = format!("{kind}plain.{rest}");
                break;
            }
        }
        out.push_str(&l);
        out.push('\n');
    }
    out
}

fn attach(c: &Db, path: &Path, read_only: bool) -> Result<()> {
    let p = path.to_str().ok_or_else(|| Error::other("bad_path", ""))?;
    let target =
        if read_only { format!("file:{}?mode=ro", p.replace('%', "%25").replace('?', "%3f").replace('#', "%23")) } else { p.to_string() };
    c.execute("ATTACH DATABASE ? AS plain KEY ''", [target])?;
    Ok(())
}

/// Writes a plain copy of this database to `path` (must not exist).
pub fn export_plain(c: &Db, path: &Path) -> Result<()> {
    attach(c, path, false)?;
    let r = tx(c, || {
        c.execute_batch(&plain_schema())?;
        for t in TABLES {
            c.execute(&format!("INSERT INTO plain.\"{t}\" SELECT * FROM main.\"{t}\""), [])?;
        }
        c.execute("INSERT INTO plain.report_text (rowid, text) SELECT rowid, text FROM main.report_text", [])?;
        Ok(())
    });
    c.execute_batch("DETACH DATABASE plain")?;
    r
}

/// (type, name) of every object a database created from the schema has.
fn expected_objects() -> Result<BTreeSet<(String, String, Option<String>)>> {
    let m = rusqlite::Connection::open_in_memory()?;
    m.execute_batch(&schema_body())?;
    objects(&m, "main")
}

/// (type, name, definition) of every object: a differing definition (a view or trigger standing in, other columns) is refused.
/// Comments and whitespace are ignored, so backups made under older comment text still match.
fn objects(c: &Db, schema: &str) -> Result<BTreeSet<(String, String, Option<String>)>> {
    let mut st = c.prepare(&format!("SELECT type, name, sql FROM {schema}.sqlite_schema"))?;
    let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, Option<String>>(2)?.map(|s| definition(&s)))))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// SQL without `--` comments, with whitespace collapsed (the schema has no `--` inside string literals).
fn definition(sql: &str) -> String {
    sql.lines().map(|l| l.split("--").next().unwrap_or("")).collect::<Vec<_>>().join(" ").split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Merges the plain database file at `path` (untrusted: checked before anything is read).
pub fn merge_from(c: &Db, path: &Path, now_ms: i64) -> Result<MergeReport> {
    c.pragma_update(None, "trusted_schema", "OFF")?;
    attach(c, path, true)?;
    let r = (|| {
        let ok: String = c.query_row("PRAGMA plain.quick_check", [], |r| r.get(0))?;
        if ok != "ok" || objects(c, "plain")? != expected_objects()? {
            return Err(Error::invalid("backup_db", "not a health database"));
        }
        tx(c, || merge_attached(c, now_ms))
    })();
    let _ = c.execute_batch(
        "DROP TABLE IF EXISTS temp.merge_metric; DROP TABLE IF EXISTS temp.merge_source; DROP TABLE IF EXISTS temp.merge_import;",
    );
    c.execute_batch("DETACH DATABASE plain")?;
    r
}

/// Clock skew tolerated between devices.
pub(crate) const MAX_SKEW_MS: i64 = 86_400_000;

/// "Larger uid wins" makes uids security-relevant: a crafted backup could otherwise take over every local row
/// (uid "zzzz") for good. Accepted: a UUIDv7 not from the future, or a signed-result id (web ingest uses the
/// result id as uid) on a lab/staff/room source only; known local uids always pass.
fn plausible_uid(c: &Db, uid: &str, source_id: i64, now_ms: i64) -> Result<bool> {
    if one::<i64>(c, "SELECT 1 FROM import WHERE uid = ?", [uid])?.is_some() {
        return Ok(true);
    }
    if let Some(t) = crate::batches::uuid_time(uid) {
        return Ok(t <= now_ms + MAX_SKEW_MS && uid.len() == 36 && uid == uid.to_ascii_lowercase());
    }
    let connector: String = c.query_row("SELECT connector FROM source WHERE id = ?", [source_id], |r| r.get(0))?;
    Ok(matches!(connector.as_str(), "lab" | "staff" | "room"))
}

struct PImport {
    id: i64,
    uid: String,
    source_id: i64,
    parser: String,
    blob: Option<String>,
    status: String,
    meta: Option<String>,
}

fn merge_attached(c: &Db, now_ms: i64) -> Result<MergeReport> {
    let mut rep = MergeReport::default();
    c.execute_batch(
        "CREATE TEMP TABLE merge_metric (p INTEGER PRIMARY KEY, l INTEGER NOT NULL);
         CREATE TEMP TABLE merge_source (p INTEGER PRIMARY KEY, l INTEGER NOT NULL);
         CREATE TEMP TABLE merge_import (p INTEGER PRIMARY KEY, l INTEGER NOT NULL);",
    )?;

    // 1. metrics by code
    let remote: Vec<(i64, String, String, String, String)> = c
        .prepare("SELECT id, code, name, unit, agg FROM plain.metric ORDER BY id")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?
        .collect::<Result<_, _>>()?;
    let mut map_metric: HashMap<i64, i64> = HashMap::new();
    for (pid, code, name, unit, agg) in remote {
        let local: Option<(i64, String, String)> =
            c.query_row("SELECT id, unit, agg FROM metric WHERE code = ?", [&code], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).ok();
        let lid = match local {
            None => {
                c.execute("INSERT INTO metric (code, name, unit, agg) VALUES (?, ?, ?, ?)", [&code, &name, &unit, &agg])?;
                rep.added_metrics += 1;
                c.last_insert_rowid()
            }
            // user-made x:ocr metrics keep the printed spelling of their unit
            Some((lid, lunit, lagg))
                if lagg == agg && (lunit == unit || (code.starts_with("x:ocr/") && crate::units::same_unit(&lunit, &unit))) =>
            {
                lid
            }
            Some(_) => {
                rep.skipped_metrics.push(code);
                continue;
            }
        };
        c.execute("INSERT INTO merge_metric VALUES (?, ?)", [pid, lid])?;
        map_metric.insert(pid, lid);
    }
    c.execute(
        "INSERT OR IGNORE INTO metric_alias (alias, unit, metric_id) SELECT a.alias, a.unit, mm.l FROM plain.metric_alias a JOIN merge_metric mm ON mm.p = a.metric_id",
        [],
    )?;

    // 2. sources by identity; a local source keeps its own name and priority
    let remote: Vec<(i64, String, String, String, i64, Option<String>)> = c
        .prepare("SELECT id, connector, external_id, name, priority, meta FROM plain.source ORDER BY id")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))?
        .collect::<Result<_, _>>()?;
    let mut map_source: HashMap<i64, i64> = HashMap::new();
    for (pid, connector, external_id, name, priority, meta) in remote {
        let lid = match one::<i64>(c, "SELECT id FROM source WHERE connector = ? AND external_id = ?", [&connector, &external_id])? {
            Some(id) => id,
            None => {
                c.execute(
                    "INSERT INTO source (connector, external_id, name, priority, meta) VALUES (?, ?, ?, ?, ?)",
                    rusqlite::params![connector, external_id, name, priority, meta],
                )?;
                rep.added_sources += 1;
                c.last_insert_rowid()
            }
        };
        c.execute("INSERT INTO merge_source VALUES (?, ?)", [pid, lid])?;
        map_source.insert(pid, lid);
    }

    // 3. imports by uid
    let translate = |meta: Option<String>| -> (serde_json::Map<String, serde_json::Value>, Span) {
        let mut m = parse_meta(meta);
        let span: Span = span_of(&m).into_iter().filter_map(|(k, v)| map_metric.get(&k).map(|l| (*l, v))).collect();
        set_span(&mut m, &span);
        (m, span)
    };
    let remote: Vec<PImport> = c
        .prepare("SELECT id, uid, source_id, parser, blob, status, meta FROM plain.import WHERE status IN ('done', 'erased', 'review') ORDER BY id")?
        .query_map([], |r| Ok(PImport { id: r.get(0)?, uid: r.get(1)?, source_id: r.get(2)?, parser: r.get(3)?, blob: r.get(4)?, status: r.get(5)?, meta: r.get(6)? }))?
        .collect::<Result<_, _>>()?;
    let mut new_imports: Vec<(i64, Span)> = Vec::new();
    let mut touched: Vec<(i64, Span)> = Vec::new(); // (local source id, span) to rebuild
    const NEXT_ID: &str = "(SELECT coalesce(max(id), 0) + 1 FROM import)";
    for i in remote {
        let Some(&sid) = map_source.get(&i.source_id) else { continue };
        if !plausible_uid(c, &i.uid, sid, now_ms)? {
            rep.refused_imports += 1;
            continue;
        }
        let local: Option<(i64, String, Option<String>, i64)> = c
            .query_row("SELECT id, status, meta, source_id FROM import WHERE uid = ?", [&i.uid], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })
            .ok();
        match local {
            None => {
                if i.status == "erased" {
                    // keep the tombstone so an older copy cannot bring the data back later
                    c.execute(
                        &format!("INSERT INTO import (id, uid, source_id, parser, blob, status, meta) VALUES ({NEXT_ID}, ?, ?, ?, NULL, 'erased', NULL)"),
                        rusqlite::params![i.uid, sid, i.parser],
                    )?;
                    rep.added_imports += 1;
                    continue;
                }
                let mut blob = i.blob.clone();
                if let Some(b) = &blob
                    && one::<i64>(c, "SELECT 1 FROM import WHERE blob = ?", [b])?.is_some()
                {
                    blob = None;
                    rep.dropped_blobs += 1;
                }
                if i.status == "review" {
                    // no rows yet; candidates use global codes, so meta travels as is
                    c.execute(
                        &format!("INSERT INTO import (id, uid, source_id, parser, blob, status, meta) VALUES ({NEXT_ID}, ?, ?, ?, ?, 'review', ?)"),
                        rusqlite::params![i.uid, sid, i.parser, blob, i.meta],
                    )?;
                    rep.added_imports += 1;
                    rep.needed_blobs.extend(blob);
                    continue;
                }
                let (meta, span) = translate(i.meta.clone());
                c.execute(
                    &format!(
                        "INSERT INTO import (id, uid, source_id, parser, blob, status, meta) VALUES ({NEXT_ID}, ?, ?, ?, ?, 'done', ?)"
                    ),
                    rusqlite::params![i.uid, sid, i.parser, blob, serde_json::Value::Object(meta).to_string()],
                )?;
                let lid: i64 = c.query_row("SELECT id FROM import WHERE uid = ?", [&i.uid], |r| r.get(0))?;
                c.execute("INSERT INTO merge_import VALUES (?, ?)", [i.id, lid])?;
                new_imports.push((lid, span.clone()));
                touched.push((sid, span));
                rep.added_imports += 1;
                rep.needed_blobs.extend(blob);
            }
            // pending or done here, gone there: erase here too
            Some((lid, ls, _, _)) if (ls == "done" || ls == "review") && i.status == "erased" => {
                rep.orphan_blobs.extend(crate::erase::erase(c, lid, now_ms)?.orphan_blobs);
                rep.erased_imports += 1;
            }
            Some((lid, ls, lmeta, lsid)) if ls == "done" && i.status == "done" => {
                // the same import on both sides: rows merge by the usual rule, spans unite
                c.execute("INSERT INTO merge_import VALUES (?, ?)", [i.id, lid])?;
                let (_, t) = translate(i.meta.clone());
                let mut lm = parse_meta(lmeta);
                let mut cur = span_of(&lm);
                for (k, (a, b)) in &t {
                    let e = cur.entry(*k).or_insert((*a, *b));
                    *e = (e.0.min(*a), e.1.max(*b));
                }
                set_span(&mut lm, &cur);
                c.execute("UPDATE import SET meta = ? WHERE id = ?", rusqlite::params![serde_json::Value::Object(lm).to_string(), lid])?;
                touched.push((lsid, t));
            }
            // locally erased stays erased; running / failed / a review not erased there: untouched
            Some(_) => {}
        }
    }

    // 4. observations: translate ids, larger uid wins
    c.execute(
        "INSERT INTO observation (metric_id, start_ms, source_id, end_ms, tz_min, value, import_id, detail)
         SELECT mm.l, o.start_ms, ms.l, o.end_ms, o.tz_min, o.value, mi.l, o.detail
           FROM plain.observation o
           JOIN merge_metric mm ON mm.p = o.metric_id
           JOIN merge_source ms ON ms.p = o.source_id
           JOIN merge_import mi ON mi.p = o.import_id
          WHERE true
         ON CONFLICT (metric_id, start_ms, source_id) DO UPDATE SET
           end_ms = excluded.end_ms, tz_min = excluded.tz_min, value = excluded.value, import_id = excluded.import_id, detail = excluded.detail
         WHERE (SELECT uid FROM import WHERE id = excluded.import_id) >= (SELECT uid FROM import WHERE id = observation.import_id)",
        [],
    )?;

    // 5. corrections; one from the future would beat every later local correction
    let max_at = (now_ms + MAX_SKEW_MS) << crate::correction::HLC_SHIFT;
    rep.refused_corrections = c.query_row("SELECT count(*) FROM plain.correction WHERE at > ?", [max_at], |r| r.get::<_, i64>(0))? as u64;
    c.execute(
        &format!(
            "INSERT INTO correction (metric_id, source_id, start_ms, at, value)
             SELECT mm.l, ms.l, c.start_ms, c.at, c.value
               FROM plain.correction c
               JOIN merge_metric mm ON mm.p = c.metric_id
               JOIN merge_source ms ON ms.p = c.source_id
              WHERE c.at <= ?1
             ON CONFLICT (metric_id, source_id, start_ms) DO UPDATE SET at = excluded.at, value = excluded.value WHERE {CORRECTION_WINS}"
        ),
        [max_at],
    )?;

    // 6. genotypes and full text of new imports
    c.execute(
        "INSERT OR IGNORE INTO genotype (import_id, chrom, pos, rsid, genotype)
         SELECT mi.l, g.chrom, g.pos, g.rsid, g.genotype FROM plain.genotype g JOIN merge_import mi ON mi.p = g.import_id",
        [],
    )?;
    for (lid, span) in &new_imports {
        c.execute("INSERT INTO report_text (rowid, text) SELECT mi.l, t.text FROM plain.report_text t JOIN merge_import mi ON mi.p = t.rowid WHERE mi.l = ?", [lid])?;
        // files that waveform pointers need
        for (mid, (a, b)) in span {
            if one::<String>(c, "SELECT agg FROM metric WHERE id = ?", [mid])?.as_deref() != Some("none") {
                continue;
            }
            let mut st =
                c.prepare("SELECT DISTINCT value FROM observation WHERE metric_id = ? AND start_ms BETWEEN ? AND ? AND import_id = ?")?;
            for v in st.query_map([*mid, *a, *b, *lid], |r| r.get::<_, crate::Value>(0))? {
                if let crate::Value::Text(s) = v?
                    && crate::blob::is_sha(&s)
                {
                    rep.needed_blobs.push(s);
                }
            }
        }
    }

    // 7. rebuild affected daily rows (erase already rebuilt its own)
    let scopes: Vec<Scope> = touched
        .iter()
        .flat_map(|(sid, span)| span.iter().map(move |(m, (a, b))| Scope { metric_id: *m, source_id: *sid, from: *a, to: *b }))
        .collect();
    crate::daily::rebuild(c, &scopes, now_ms)?;
    c.execute_batch("DROP TABLE merge_metric; DROP TABLE merge_source; DROP TABLE merge_import;")?;
    rep.needed_blobs.sort();
    rep.needed_blobs.dedup();
    // erase (step 3) judged orphans before later steps added rows; a file referenced again must stay
    let mut orphans = Vec::new();
    for sha in std::mem::take(&mut rep.orphan_blobs) {
        if !crate::erase::referenced(c, &sha)? && !orphans.contains(&sha) {
            orphans.push(sha);
        }
    }
    rep.orphan_blobs = orphans;
    Ok(rep)
}
