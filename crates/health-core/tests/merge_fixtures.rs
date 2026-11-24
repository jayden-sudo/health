//! Shared merge fixtures (docs/merge-fixes.md §9): the web runs the same cases. Each case builds two databases from
//! rows given by identity (never local ids), merges a ← b and b ← a from the originals, requires both results to
//! equal `expect`, merges once more both ways without change, and checks that `daily` equals a full rebuild.
//! Expectations are derived by hand from the rules, never generated from this implementation.

use health_core::db::Db;
use health_core::keys::MemoryStore;
use health_core::profile::Profile;
use health_core::write::{Span, parse_meta, set_span};
use health_core::{Value, daily, merge};
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const NOW: i64 = 1_795_176_000_000;

/// A typed value: JSON can't tell 5 from 5.0.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Typed {
    t: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    v: Option<Json>,
}

impl Typed {
    fn to_value(&self) -> Value {
        match (self.t.as_str(), &self.v) {
            ("integer", Some(v)) => Value::Int(v.as_i64().expect("integer")),
            ("real", Some(v)) => Value::Real(v.as_f64().expect("real")),
            ("text", Some(v)) => Value::Text(v.as_str().expect("text").into()),
            ("null", _) => Value::Null,
            other => panic!("bad typed value {other:?}"),
        }
    }
    fn of(v: Value) -> Typed {
        match v {
            Value::Int(i) => Typed { t: "integer".into(), v: Some(json!(i)) },
            Value::Real(r) => Typed { t: "real".into(), v: Some(json!(r)) },
            Value::Text(s) => Typed { t: "text".into(), v: Some(json!(s)) },
            Value::Null => Typed { t: "null".into(), v: None },
        }
    }
}

#[derive(Debug, Clone, PartialEq, PartialOrd, Eq, Ord, Serialize, Deserialize)]
struct Src {
    connector: String,
    external_id: String,
    name: String,
    priority: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    set_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    set_by: Option<String>,
}

#[derive(Debug, Clone, PartialEq, PartialOrd, Eq, Ord, Serialize, Deserialize)]
struct Imp {
    uid: String,
    connector: String,
    external_id: String,
    status: String,
    #[serde(default)]
    blob: Option<String>,
    /// `meta.file`: the file this import brought (R6b); part of the target property
    #[serde(default, skip_serializing_if = "Option::is_none")]
    file: Option<String>,
    /// Built as given when present (else from the rows); compared only in cases with `check_spans`: code → [from, to]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    span: Option<BTreeMap<String, (i64, i64)>>,
}

/// A measure made on a device (x:ocr/…), created in the order given, so local ids can differ between devices.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Met {
    code: String,
    name: String,
    unit: String,
    agg: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Obs {
    code: String,
    connector: String,
    external_id: String,
    start_ms: i64,
    uid: String,
    #[serde(default)]
    end_ms: Option<i64>,
    tz_min: i64,
    value: Typed,
    #[serde(default)]
    detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Corr {
    code: String,
    connector: String,
    external_id: String,
    start_ms: i64,
    /// HLC as a decimal string
    at: String,
    value: Typed,
}

#[derive(Debug, Clone, PartialEq, PartialOrd, Eq, Ord, Serialize, Deserialize)]
struct Geno {
    uid: String,
    chrom: String,
    pos: i64,
    #[serde(default)]
    rsid: Option<i64>,
    genotype: String,
}

#[derive(Debug, Clone, PartialEq, PartialOrd, Eq, Ord, Serialize, Deserialize)]
struct Text {
    uid: String,
    text: String,
}

#[derive(Debug, Clone, PartialEq, PartialOrd, Eq, Ord, Serialize, Deserialize)]
struct Alias {
    alias: String,
    unit: String,
    code: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Rows {
    /// Built only, never compared (metric spelling is outside the target property, G4)
    #[serde(default, skip_serializing)]
    metrics: Vec<Met>,
    #[serde(default)]
    sources: Vec<Src>,
    #[serde(default)]
    imports: Vec<Imp>,
    #[serde(default)]
    obs: Vec<Obs>,
    #[serde(default)]
    corrections: Vec<Corr>,
    #[serde(default)]
    genotypes: Vec<Geno>,
    #[serde(default)]
    texts: Vec<Text>,
    #[serde(default)]
    aliases: Vec<Alias>,
}

impl Rows {
    /// Sorted by identity, so two dumps compare by content.
    fn canonical(mut self) -> Rows {
        self.sources.sort();
        self.imports.sort();
        let key = |o: &Obs| (o.code.clone(), o.connector.clone(), o.external_id.clone(), o.start_ms);
        self.obs.sort_by_key(key);
        self.corrections.sort_by_key(|c| (c.code.clone(), c.connector.clone(), c.external_id.clone(), c.start_ms));
        self.genotypes.sort();
        self.texts.sort();
        self.aliases.sort();
        self
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    name: String,
    #[serde(default)]
    about: String,
    a: Rows,
    b: Rows,
    expect: Rows,
    /// When b ends differently from a by design (G1: a refused correction stays on the device that made it)
    #[serde(default)]
    expect_b: Option<Rows>,
    /// Exchange rounds before comparing (some rules converge in two, e.g. a remote NULL blob)
    #[serde(default = "one")]
    rounds: u32,
    #[serde(default)]
    check_spans: bool,
}

fn one() -> u32 {
    1
}

fn metric_id(c: &Db, code: &str) -> i64 {
    c.query_row("SELECT id FROM metric WHERE code = ?", [code], |r| r.get(0)).unwrap_or_else(|_| panic!("no metric {code}"))
}

fn source_id(c: &Db, connector: &str, external_id: &str) -> i64 {
    c.query_row("SELECT id FROM source WHERE connector = ? AND external_id = ?", [connector, external_id], |r| r.get(0))
        .unwrap_or_else(|_| panic!("no source {connector}/{external_id}"))
}

/// Builds a profile's tables from rows given by identity.
fn build(c: &Db, rows: &Rows) {
    for m in &rows.metrics {
        c.execute("INSERT INTO metric (code, name, unit, agg) VALUES (?, ?, ?, ?)", rusqlite::params![m.code, m.name, m.unit, m.agg])
            .unwrap();
    }
    for s in &rows.sources {
        let mut meta = serde_json::Map::new();
        if let Some(a) = &s.set_at {
            meta.insert("set_at".into(), json!(a));
        }
        if let Some(b) = &s.set_by {
            meta.insert("set_by".into(), json!(b));
        }
        let meta = (!meta.is_empty()).then(|| Json::Object(meta).to_string());
        c.execute(
            "INSERT INTO source (connector, external_id, name, priority, meta) VALUES (?, ?, ?, ?, ?)",
            rusqlite::params![s.connector, s.external_id, s.name, s.priority, meta],
        )
        .unwrap();
    }
    for i in &rows.imports {
        let sid = source_id(c, &i.connector, &i.external_id);
        // spans from the rows this import owns
        let mut span: Span = BTreeMap::new();
        match &i.span {
            Some(given) => {
                for (code, v) in given {
                    span.insert(metric_id(c, code), *v);
                }
            }
            None => {
                for o in rows.obs.iter().filter(|o| o.uid == i.uid) {
                    let e = span.entry(metric_id(c, &o.code)).or_insert((o.start_ms, o.start_ms));
                    *e = (e.0.min(o.start_ms), e.1.max(o.start_ms));
                }
            }
        }
        let mut m = parse_meta(None);
        match i.status.as_str() {
            "erased" => {}
            "review" => {
                m.insert("candidates".into(), json!({ "items": [] }));
            }
            _ => set_span(&mut m, &span),
        }
        if let Some(f) = &i.file {
            m.insert("file".into(), json!(f));
        }
        let meta = (i.status != "erased").then(|| Json::Object(m).to_string());
        c.execute(
            "INSERT INTO import (uid, source_id, parser, blob, status, meta) VALUES (?, ?, 'fixture', ?, ?, ?)",
            rusqlite::params![i.uid, sid, i.blob, i.status, meta],
        )
        .unwrap();
    }
    let imp = |uid: &str| -> i64 { c.query_row("SELECT id FROM import WHERE uid = ?", [uid], |r| r.get(0)).unwrap() };
    for o in &rows.obs {
        c.execute(
            "INSERT INTO observation (metric_id, start_ms, source_id, end_ms, tz_min, value, import_id, detail) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            rusqlite::params![
                metric_id(c, &o.code),
                o.start_ms,
                source_id(c, &o.connector, &o.external_id),
                o.end_ms,
                o.tz_min,
                o.value.to_value(),
                imp(&o.uid),
                o.detail
            ],
        )
        .unwrap();
    }
    for k in &rows.corrections {
        c.execute(
            "INSERT INTO correction (metric_id, source_id, start_ms, at, value) VALUES (?, ?, ?, ?, ?)",
            rusqlite::params![
                metric_id(c, &k.code),
                source_id(c, &k.connector, &k.external_id),
                k.start_ms,
                k.at.parse::<i64>().unwrap(),
                k.value.to_value()
            ],
        )
        .unwrap();
    }
    for g in &rows.genotypes {
        c.execute(
            "INSERT INTO genotype (import_id, chrom, pos, rsid, genotype) VALUES (?, ?, ?, ?, ?)",
            rusqlite::params![imp(&g.uid), g.chrom, g.pos, g.rsid, g.genotype],
        )
        .unwrap();
    }
    for t in &rows.texts {
        c.execute("INSERT INTO report_text (rowid, text) VALUES (?, ?)", rusqlite::params![imp(&t.uid), t.text]).unwrap();
    }
    for a in &rows.aliases {
        c.execute(
            "INSERT INTO metric_alias (alias, unit, metric_id) VALUES (?, ?, ?)",
            rusqlite::params![a.alias, a.unit, metric_id(c, &a.code)],
        )
        .unwrap();
    }
    daily::rebuild_all(c, NOW).unwrap();
}

/// The tables by identity. Catalog aliases are included only if a case created any (the catalog seeds none).
fn dump(c: &Db, spans: bool) -> Rows {
    let q = |sql: &str| c.prepare(sql).unwrap();
    let sources = q("SELECT connector, external_id, name, priority, meta FROM source")
        .query_map([], |r| {
            let m = parse_meta(r.get(4)?);
            Ok(Src {
                connector: r.get(0)?,
                external_id: r.get(1)?,
                name: r.get(2)?,
                priority: r.get(3)?,
                set_at: m.get("set_at").and_then(Json::as_str).map(String::from),
                set_by: m.get("set_by").and_then(Json::as_str).map(String::from),
            })
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let code_of = |id: i64| -> String { c.query_row("SELECT code FROM metric WHERE id = ?", [id], |r| r.get(0)).unwrap() };
    let imports: Vec<Imp> =
        q("SELECT i.uid, s.connector, s.external_id, i.status, i.blob, i.meta FROM import i JOIN source s ON s.id = i.source_id
                      WHERE i.status IN ('done', 'review', 'erased')")
        .query_map([], |r| {
            Ok((
                Imp {
                    uid: r.get(0)?,
                    connector: r.get(1)?,
                    external_id: r.get(2)?,
                    status: r.get(3)?,
                    blob: r.get(4)?,
                    file: None,
                    span: None,
                },
                r.get::<_, Option<String>>(5)?,
            ))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
        .into_iter()
        .map(|(mut i, meta)| {
            let m = parse_meta(meta);
            i.file = m.get("file").and_then(Json::as_str).map(String::from);
            if spans && i.status == "done" {
                i.span = Some(health_core::write::span_of(&m).into_iter().map(|(m, v)| (code_of(m), v)).collect());
            }
            i
        })
        .collect();
    let obs = q("SELECT m.code, s.connector, s.external_id, o.start_ms, i.uid, o.end_ms, o.tz_min, o.value, o.detail
                   FROM observation o JOIN metric m ON m.id = o.metric_id JOIN source s ON s.id = o.source_id
                   JOIN import i ON i.id = o.import_id")
    .query_map([], |r| {
        Ok(Obs {
            code: r.get(0)?,
            connector: r.get(1)?,
            external_id: r.get(2)?,
            start_ms: r.get(3)?,
            uid: r.get(4)?,
            end_ms: r.get(5)?,
            tz_min: r.get(6)?,
            value: Typed::of(r.get(7)?),
            detail: r.get(8)?,
        })
    })
    .unwrap()
    .collect::<Result<_, _>>()
    .unwrap();
    let corrections = q("SELECT m.code, s.connector, s.external_id, k.start_ms, k.at, k.value
                           FROM correction k JOIN metric m ON m.id = k.metric_id JOIN source s ON s.id = k.source_id")
    .query_map([], |r| {
        Ok(Corr {
            code: r.get(0)?,
            connector: r.get(1)?,
            external_id: r.get(2)?,
            start_ms: r.get(3)?,
            at: r.get::<_, i64>(4)?.to_string(),
            value: Typed::of(r.get(5)?),
        })
    })
    .unwrap()
    .collect::<Result<_, _>>()
    .unwrap();
    let genotypes = q("SELECT i.uid, g.chrom, g.pos, g.rsid, g.genotype FROM genotype g JOIN import i ON i.id = g.import_id")
        .query_map([], |r| Ok(Geno { uid: r.get(0)?, chrom: r.get(1)?, pos: r.get(2)?, rsid: r.get(3)?, genotype: r.get(4)? }))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let texts = q("SELECT i.uid, t.text FROM report_text t JOIN import i ON i.id = t.rowid")
        .query_map([], |r| Ok(Text { uid: r.get(0)?, text: r.get(1)? }))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let aliases = q("SELECT a.alias, a.unit, m.code FROM metric_alias a JOIN metric m ON m.id = a.metric_id")
        .query_map([], |r| Ok(Alias { alias: r.get(0)?, unit: r.get(1)?, code: r.get(2)? }))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    Rows { metrics: vec![], sources, imports, obs, corrections, genotypes, texts, aliases }.canonical()
}

fn daily_rows(c: &Db) -> Vec<String> {
    let mut st =
        c.prepare("SELECT metric_id, day, source_id, n, quote(sum), quote(min), quote(max), seconds FROM daily ORDER BY 1, 2, 3").unwrap();
    st.query_map([], |r| {
        Ok(format!(
            "{} {} {} {} {} {} {} {:?}",
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?,
            r.get::<_, i64>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, String>(5)?,
            r.get::<_, String>(6)?,
            r.get::<_, Option<i64>>(7)?
        ))
    })
    .unwrap()
    .collect::<Result<_, _>>()
    .unwrap()
}

fn plain(p: &Profile, dir: &Path, name: &str) -> PathBuf {
    let f = dir.join(name);
    let _ = std::fs::remove_file(&f);
    merge::export_plain(&p.db, &f).unwrap();
    f
}

/// Runs one case; Err describes the first difference.
fn run(case: &Case) -> Result<(), String> {
    let dir = tempfile::tempdir().unwrap();
    let keys = MemoryStore::default();
    let a = Profile::init(dir.path(), "a", &keys).unwrap();
    let b = Profile::init(dir.path(), "b", &keys).unwrap();
    build(&a.db, &case.a);
    build(&b.db, &case.b);
    for round in 0..case.rounds {
        let (a0, b0) = (plain(&a, dir.path(), &format!("a{round}.db")), plain(&b, dir.path(), &format!("b{round}.db")));
        merge::merge_from(&a.db, &b0, NOW).map_err(|e| format!("a ← b: {e}"))?;
        merge::merge_from(&b.db, &a0, NOW).map_err(|e| format!("b ← a: {e}"))?;
    }
    let expect = case.expect.clone().canonical();
    let expect_b = case.expect_b.clone().map_or(expect.clone(), Rows::canonical);
    for (side, p, expect) in [("a ← b", &a, &expect), ("b ← a", &b, &expect_b)] {
        let got = dump(&p.db, case.check_spans);
        if got != *expect {
            return Err(format!(
                "{side} differs\n  got:    {}\n  expect: {}",
                serde_json::to_string(&got).unwrap(),
                serde_json::to_string(&expect).unwrap()
            ));
        }
        let before = daily_rows(&p.db);
        daily::rebuild_all(&p.db, NOW).unwrap();
        if daily_rows(&p.db) != before {
            return Err(format!("{side}: daily differs from a full rebuild"));
        }
    }
    // once more, both ways: nothing changes
    let (a1, b1) = (plain(&a, dir.path(), "again-a.db"), plain(&b, dir.path(), "again-b.db"));
    merge::merge_from(&a.db, &b1, NOW).map_err(|e| format!("again a ← b: {e}"))?;
    merge::merge_from(&b.db, &a1, NOW).map_err(|e| format!("again b ← a: {e}"))?;
    if dump(&a.db, case.check_spans) != expect {
        return Err("merging again changed a".into());
    }
    if dump(&b.db, case.check_spans) != expect_b {
        return Err("merging again changed b".into());
    }
    Ok(())
}

#[test]
fn shared_merge_fixtures() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/merge");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "json") && !p.ends_with("units.json"))
        .collect();
    files.sort();
    assert!(!files.is_empty());
    let only = std::env::var("CASE").ok();
    let mut failed = Vec::new();
    for f in files {
        let case: Case = serde_json::from_str(&std::fs::read_to_string(&f).unwrap()).unwrap_or_else(|e| panic!("{}: {e}", f.display()));
        if only.as_deref().is_some_and(|o| !case.name.contains(o)) {
            continue;
        }
        let _ = &case.about;
        if let Err(e) = run(&case) {
            failed.push(format!("{}: {e}", case.name));
        }
    }
    assert!(failed.is_empty(), "{} case(s) failed:\n{}", failed.len(), failed.join("\n\n"));
}

#[test]
fn shared_unit_pairs() {
    let f = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/merge/units.json");
    let v: Json = serde_json::from_str(&std::fs::read_to_string(f).unwrap()).unwrap();
    let mut wrong = Vec::new();
    for p in v["pairs"].as_array().unwrap() {
        let (a, b, same) = (p[0].as_str().unwrap(), p[1].as_str().unwrap(), p[2].as_bool().unwrap());
        if health_core::units::same_unit(a, b) != same {
            wrong.push(format!("{a:?} vs {b:?}: expected same = {same} ({})", p[3]));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}
