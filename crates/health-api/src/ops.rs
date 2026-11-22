//! Operation definitions: one typed fn per op, registered in `OPS`.

use crate::{Ctx, Delegate, OpDef, bad_request};
use health_core::access::{Audit, Grant, GrantSpec};
use health_core::backup::{self, Exported, Imported};
use health_core::batches::{self, Batch, BatchDetail};
use health_core::correction::{self, Key};
use health_core::csv::GroupCount;
use health_core::derived::{Derive, Derived};
use health_core::erase::EraseResult;
use health_core::import::Imported as FileImported;
use health_core::manual::{self, Entered, Entry};
use health_core::query::{self, Day, Metric, MetricInfo, Point, ReportHit, Source};
use health_core::review::{self, Alias, Confirmed, Proposed};
use health_core::write::ObsRow;
use health_core::{Error, Result, Value};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

macro_rules! op {
    ($name:literal, $about:literal, write: $w:expr, delegate: $d:ident, manual_tx: $m:expr, $f:path, $req:ty, $resp:ty) => {
        OpDef {
            name: $name,
            about: $about,
            write: $w,
            delegate: Delegate::$d,
            manual_tx: $m,
            req: crate::schema_of::<$req>,
            resp: crate::schema_of::<$resp>,
            run: |ctx, v| {
                let req: $req = serde_json::from_value(v).map_err(bad_request)?;
                let resp: $resp = $f(ctx, req)?;
                Ok(serde_json::to_value(resp)?)
            },
        }
    };
}

pub static OPS: &[OpDef] = &[
    op!("status", "Profile and database summary", write: false, delegate: Never, manual_tx: false, status, Empty, Status),
    op!("metric.search", "Search the metric catalog by code or name", write: false, delegate: Read, manual_tx: false, metric_search, MetricSearch, Vec<Metric>),
    op!("metric.show", "One metric with its accepted units", write: false, delegate: Read, manual_tx: false, metric_show, CodeReq, MetricInfo),
    op!("manual.enter", "Enter values by hand (one import per call)", write: true, delegate: Never, manual_tx: false, manual_enter, ManualEnter, Entered),
    op!("manual.edit", "Replace a manual entry", write: true, delegate: Never, manual_tx: false, manual_edit, ManualEdit, Entered),
    op!("manual.remove", "Delete a manual entry", write: true, delegate: Never, manual_tx: false, manual_remove, PointReq, Done),
    op!("obs.query", "Points of one metric in a time range (UTC ms, [from, to))", write: false, delegate: Read, manual_tx: false, obs_query, ObsQuery, Vec<Point>),
    op!("obs.latest", "Newest point per metric", write: false, delegate: Read, manual_tx: false, obs_latest, Latest, Vec<Point>),
    op!("daily.query", "Daily summaries of one metric (one elected source per day)", write: false, delegate: Read, manual_tx: false, daily_query, DailyQuery, Vec<Day>),
    op!("daily.rebuild", "Rebuild every daily summary", write: true, delegate: Never, manual_tx: false, daily_rebuild, Empty, Done),
    op!("correct", "Correct or delete (value null) a point from a device, lab or lab photo", write: true, delegate: Never, manual_tx: false, correct, Correct, Corrected),
    op!("source.list", "Data sources and their election priority", write: false, delegate: Read, manual_tx: false, source_list, Empty, Vec<Source>),
    op!("source.update", "Rename a source or change its priority (smaller wins)", write: true, delegate: Never, manual_tx: false, source_update, SourceUpdate, Source),
    op!("import.list", "Import batches, newest first", write: false, delegate: Never, manual_tx: false, import_list, ImportList, Vec<Batch>),
    op!("import.show", "One batch and the rows it holds now", write: false, delegate: Never, manual_tx: false, import_show, IdLimit, BatchDetail),
    op!("import.erase", "Delete a batch and everything it delivered", write: true, delegate: Never, manual_tx: false, import_erase, IdReq, EraseResult),
    op!("review.list", "Batches waiting for review (invisible until confirmed)", write: false, delegate: Never, manual_tx: false, review_list, Empty, Vec<BatchDetail>),
    op!("review.confirm", "Confirm a review batch with the rows to write", write: true, delegate: Never, manual_tx: false, review_confirm, ReviewConfirm, Confirmed),
    op!("review.discard", "Discard a review batch", write: true, delegate: Never, manual_tx: false, review_discard, IdReq, Done),
    op!("report.search", "Full-text search of confirmed reports", write: false, delegate: Read, manual_tx: false, report_search, ReportSearch, Vec<ReportHit>),
    op!("report.show", "Full text of one report", write: false, delegate: Read, manual_tx: false, report_show, IdReq, ReportText),
    op!("backup.export", "Write an encrypted backup (zu-health-export v1)", write: true, delegate: Never, manual_tx: true, backup_export, BackupExport, Exported),
    op!("backup.import", "Merge an encrypted backup (never overwrites)", write: true, delegate: Never, manual_tx: true, backup_import, BackupImport, Imported),
    op!("grant.create", "Give an agent a token for a limited view (shown once)", write: false, delegate: Never, manual_tx: false, grant_create, GrantSpec, GrantCreated),
    op!("grant.list", "Grants and their state (never the tokens)", write: false, delegate: Never, manual_tx: false, grant_list, Empty, Vec<Grant>),
    op!("grant.revoke", "Revoke a grant now", write: false, delegate: Never, manual_tx: false, grant_revoke, IdReq, Done),
    op!("audit.list", "Who read what, when (no values)", write: false, delegate: Never, manual_tx: false, audit_list, AuditList, Vec<Audit>),
    op!("whoami", "This token's grant: groups, window, detail, writes, expiry", write: false, delegate: Read, manual_tx: false, whoami, Empty, WhoAmI),
    op!("review.propose", "Propose rows; they stay invisible until the person confirms them", write: true, delegate: Propose, manual_tx: false, review_propose, Propose, Proposed),
    op!("derived.write", "Write an algorithm's results as x:derived/<algo>/<name> (replaces this writer's earlier run)", write: true, delegate: Derived, manual_tx: false, derived_write, Derive, Derived),
    op!("import.apple", "Import an Apple Health export zip (one batch per device)", write: true, delegate: Never, manual_tx: false, import_apple, PathReq, FileImported),
    op!("import.whoop", "Import a saved WHOOP API bundle", write: true, delegate: Never, manual_tx: false, import_whoop, PathReq, FileImported),
    op!("import.rows", "Import JSON lines of rows from one named source", write: true, delegate: Never, manual_tx: false, import_rows, RowsFile, FileImported),
    op!("export.groups", "Rows per data group, before choosing what to export", write: false, delegate: Never, manual_tx: false, export_groups, ExportGroups, Vec<GroupCount>),
    op!("export.csv", "Write an unencrypted CSV for external analysis (chosen groups only)", write: false, delegate: Never, manual_tx: false, export_csv, ExportCsv, ExportedCsv),
];

// ---------- request / response types ----------

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Empty {}

#[derive(Serialize, JsonSchema)]
pub struct Done {
    pub ok: bool,
}
const DONE: Done = Done { ok: true };

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CodeReq {
    /// metric.code, e.g. "loinc:2345-7"
    pub code: String,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IdReq {
    pub id: i64,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IdLimit {
    pub id: i64,
    #[serde(default = "fifty")]
    pub limit: i64,
}
fn fifty() -> i64 {
    50
}
fn twenty() -> i64 {
    20
}

#[derive(Serialize, JsonSchema)]
pub struct Status {
    pub profile: String,
    pub key_store: String,
    pub schema_version: i64,
    pub metrics: i64,
    pub sources: i64,
    pub imports: i64,
    pub reviews: i64,
    pub observations: i64,
    pub db_bytes: u64,
}

fn status(ctx: &mut Ctx, _: Empty) -> Result<Status> {
    let c = ctx.db();
    let n = |sql: &str| -> Result<i64> { Ok(c.query_row(sql, [], |r| r.get(0))?) };
    Ok(Status {
        profile: ctx.profile.name.clone(),
        key_store: ctx.profile.key_store.into(),
        schema_version: n("PRAGMA user_version")?,
        metrics: n("SELECT count(*) FROM metric")?,
        sources: n("SELECT count(*) FROM source")?,
        imports: n("SELECT count(*) FROM import WHERE status = 'done'")?,
        reviews: n("SELECT count(*) FROM import WHERE status = 'review'")?,
        observations: n("SELECT count(*) FROM v_observation")?,
        db_bytes: std::fs::metadata(ctx.profile.dir.join("health.db")).map(|m| m.len()).unwrap_or(0),
    })
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MetricSearch {
    pub q: String,
    #[serde(default = "twenty")]
    pub limit: i64,
}

fn metric_search(ctx: &mut Ctx, r: MetricSearch) -> Result<Vec<Metric>> {
    query::metric_search(ctx.db(), &ctx.scope(), &r.q, r.limit)
}

fn metric_show(ctx: &mut Ctx, r: CodeReq) -> Result<MetricInfo> {
    query::metric_show(ctx.db(), &ctx.scope(), &r.code)
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ManualEnter {
    pub entries: Vec<Entry>,
}

fn manual_enter(ctx: &mut Ctx, r: ManualEnter) -> Result<Entered> {
    if r.entries.is_empty() {
        return Err(Error::invalid("bad_request", "no entries"));
    }
    manual::enter(ctx.db(), &r.entries, ctx.now_ms)
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PointReq {
    /// metric.code of the entry
    pub code: String,
    pub start_ms: i64,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ManualEdit {
    pub code: String,
    pub start_ms: i64,
    pub entry: Entry,
}

fn manual_edit(ctx: &mut Ctx, r: ManualEdit) -> Result<Entered> {
    manual::edit(ctx.db(), &r.code, r.start_ms, &r.entry, ctx.now_ms)
}

fn manual_remove(ctx: &mut Ctx, r: PointReq) -> Result<Done> {
    manual::remove(ctx.db(), &r.code, r.start_ms, ctx.now_ms)?;
    Ok(DONE)
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ObsQuery {
    pub metric: String,
    #[serde(default)]
    pub from_ms: Option<i64>,
    #[serde(default)]
    pub to_ms: Option<i64>,
    #[serde(default)]
    pub source_id: Option<i64>,
    #[serde(default = "thousand")]
    pub limit: i64,
    /// Take `limit` points from the newest end (returned newest first)
    #[serde(default)]
    pub newest_first: bool,
}
fn thousand() -> i64 {
    1000
}

fn obs_query(ctx: &mut Ctx, r: ObsQuery) -> Result<Vec<Point>> {
    query::obs_query_ordered(ctx.db(), &ctx.scope(), &r.metric, r.from_ms, r.to_ms, r.source_id, r.limit, r.newest_first)
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Latest {
    #[serde(default)]
    pub metric: Option<String>,
}

fn obs_latest(ctx: &mut Ctx, r: Latest) -> Result<Vec<Point>> {
    query::latest(ctx.db(), &ctx.scope(), r.metric.as_deref())
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DailyQuery {
    pub metric: String,
    /// First local day, YYYY-MM-DD (inclusive)
    #[serde(default)]
    pub from: Option<String>,
    /// Last local day, YYYY-MM-DD (inclusive)
    #[serde(default)]
    pub to: Option<String>,
}

fn daily_query(ctx: &mut Ctx, r: DailyQuery) -> Result<Vec<Day>> {
    query::daily(ctx.db(), &ctx.scope(), &r.metric, r.from.as_deref(), r.to.as_deref())
}

fn daily_rebuild(ctx: &mut Ctx, _: Empty) -> Result<Done> {
    health_core::daily::rebuild_all(ctx.db(), ctx.now_ms)?;
    Ok(DONE)
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Correct {
    pub metric: String,
    pub source_id: i64,
    pub start_ms: i64,
    /// New value in the metric's canonical unit; null deletes the point
    pub value: Value,
}

#[derive(Serialize, JsonSchema)]
pub struct Corrected {
    pub applied: bool,
    /// HLC timestamp (decimal string: it exceeds 2^53)
    pub at: String,
}

fn correct(ctx: &mut Ctx, r: Correct) -> Result<Corrected> {
    let c = ctx.db();
    let found: Option<(i64, String)> = c
        .query_row(
            "SELECT o.metric_id, s.connector FROM observation o JOIN metric m ON m.id = o.metric_id JOIN source s ON s.id = o.source_id
              WHERE m.code = ? AND o.source_id = ? AND o.start_ms = ?",
            health_core::rusqlite::params![r.metric, r.source_id, r.start_ms],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .ok();
    let (metric_id, connector) = found.ok_or_else(|| Error::not_found("no_row", format!("{} {} {}", r.metric, r.source_id, r.start_ms)))?;
    // Manual entries are the person's own: edited, not corrected (DB design §4.6). Confirmed lab-photo values
    // go through the correction layer, so the printed original stays visible (owner, 2026-11-22).
    if connector == "manual" {
        return Err(Error::invalid("use_edit", "manual entries are changed with manual.edit / manual.remove"));
    }
    let key = Key { metric_id, source_id: r.source_id, start_ms: r.start_ms };
    let at = correction::next_at(c, ctx.now_ms)?;
    let applied = correction::put(c, key, at, &r.value, ctx.now_ms)?;
    Ok(Corrected { applied, at: at.to_string() })
}

fn source_list(ctx: &mut Ctx, _: Empty) -> Result<Vec<Source>> {
    let all = query::sources(ctx.db())?;
    if ctx.grant().is_none() {
        return Ok(all);
    }
    // Delegates: only {id, connector} of sources with daily data in scope; device ids never leave (grading §2.3).
    let ids: Vec<i64> = match ctx.scope().ids(ctx.db())? {
        Some(json) => ctx
            .db()
            .prepare("SELECT DISTINCT source_id FROM daily WHERE metric_id IN (SELECT value FROM json_each(?))")?
            .query_map([json], |r| r.get(0))?
            .collect::<std::result::Result<_, _>>()?,
        None => all.iter().map(|s| s.id).collect(),
    };
    Ok(all
        .into_iter()
        .filter(|s| ids.contains(&s.id))
        .map(|s| Source { name: s.connector.clone(), external_id: String::new(), ..s })
        .collect())
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SourceUpdate {
    pub id: i64,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub priority: Option<i64>,
}

fn source_update(ctx: &mut Ctx, r: SourceUpdate) -> Result<Source> {
    query::source_update(ctx.db(), r.id, r.name.as_deref(), r.priority)
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportList {
    #[serde(default)]
    pub connector: Option<String>,
}

fn import_list(ctx: &mut Ctx, r: ImportList) -> Result<Vec<Batch>> {
    batches::list(ctx.db(), r.connector.as_deref(), false)
}

fn import_show(ctx: &mut Ctx, r: IdLimit) -> Result<BatchDetail> {
    batches::show(ctx.db(), r.id, r.limit)
}

fn import_erase(ctx: &mut Ctx, r: IdReq) -> Result<EraseResult> {
    let res = batches::erase(ctx.db(), r.id, ctx.now_ms)?;
    ctx.delete_blobs_after_commit(res.orphan_blobs.clone());
    Ok(res)
}

fn review_list(ctx: &mut Ctx, _: Empty) -> Result<Vec<BatchDetail>> {
    batches::list(ctx.db(), None, true)?.into_iter().map(|b| batches::show(ctx.db(), b.import_id, 0)).collect()
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReviewConfirm {
    pub id: i64,
    /// Rows to write; any rejected row rolls the whole confirm back. Omitted for an agent's proposal: its own rows
    #[serde(default)]
    pub rows: Option<Vec<ObsRow>>,
    /// Full report text, kept for search
    #[serde(default)]
    pub text: Option<String>,
    /// Printed name + unit → metric mappings to remember
    #[serde(default)]
    pub aliases: Vec<Alias>,
}

fn review_confirm(ctx: &mut Ctx, r: ReviewConfirm) -> Result<Confirmed> {
    let rows = match r.rows {
        Some(rows) => rows,
        None => review::proposal_rows(ctx.db(), r.id)?.ok_or_else(|| Error::invalid("bad_request", "rows are required for this review"))?,
    };
    review::confirm(ctx.db(), r.id, &rows, r.text.as_deref(), &r.aliases, ctx.now_ms)
}

fn review_discard(ctx: &mut Ctx, r: IdReq) -> Result<Done> {
    let orphans = review::discard(ctx.db(), r.id, ctx.now_ms)?;
    ctx.delete_blobs_after_commit(orphans);
    Ok(DONE)
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReportSearch {
    pub q: String,
    #[serde(default = "twenty")]
    pub limit: i64,
}

fn report_search(ctx: &mut Ctx, r: ReportSearch) -> Result<Vec<ReportHit>> {
    query::report_search(ctx.db(), &ctx.scope(), &r.q, r.limit)
}

#[derive(Serialize, JsonSchema)]
pub struct ReportText {
    pub text: String,
}

fn report_show(ctx: &mut Ctx, r: IdReq) -> Result<ReportText> {
    Ok(ReportText { text: query::report_show(ctx.db(), &ctx.scope(), r.id)? })
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BackupExport {
    /// Output file (must not exist unless `replace`)
    pub path: String,
    /// Overwrite an existing file (the owner confirmed it, e.g. in a save dialog)
    #[serde(default)]
    pub replace: bool,
    /// At least 8 characters. Never logged or audited.
    pub password: String,
    /// Leave out original files (a backup sized for the web version)
    #[serde(default)]
    pub no_blobs: bool,
}

fn backup_export(ctx: &mut Ctx, r: BackupExport) -> Result<Exported> {
    backup::export_to(ctx.profile, r.path.as_ref(), &r.password, !r.no_blobs, r.replace, ctx.now_ms, Default::default())
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BackupImport {
    pub path: String,
    pub password: String,
}

fn backup_import(ctx: &mut Ctx, r: BackupImport) -> Result<Imported> {
    backup::import(ctx.profile, r.path.as_ref(), &r.password, ctx.now_ms)
}

#[derive(Serialize, JsonSchema)]
pub struct GrantCreated {
    #[serde(flatten)]
    pub grant: Grant,
    /// Shown once; give it to the agent (HEALTH_TOKEN)
    pub token: String,
}

fn grant_create(ctx: &mut Ctx, r: GrantSpec) -> Result<GrantCreated> {
    let (grant, token) = ctx.profile.access.create(&r, ctx.now_ms)?;
    Ok(GrantCreated { grant, token })
}

fn grant_list(ctx: &mut Ctx, _: Empty) -> Result<Vec<Grant>> {
    ctx.profile.access.list()
}

fn grant_revoke(ctx: &mut Ctx, r: IdReq) -> Result<Done> {
    ctx.profile.access.revoke(r.id, ctx.now_ms)?;
    Ok(DONE)
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AuditList {
    #[serde(default)]
    pub grant_id: Option<i64>,
    #[serde(default = "fifty")]
    pub limit: i64,
}

fn audit_list(ctx: &mut Ctx, r: AuditList) -> Result<Vec<Audit>> {
    ctx.profile.access.audit(r.grant_id, r.limit)
}

#[derive(Serialize, JsonSchema)]
pub struct WhoAmI {
    pub grant: Grant,
    /// Window start (UTC ms), absent = no limit
    pub from_ms: Option<i64>,
    pub to_ms: Option<i64>,
}

fn whoami(ctx: &mut Ctx, _: Empty) -> Result<WhoAmI> {
    let g = ctx.grant().cloned().ok_or_else(|| Error::invalid("bad_request", "whoami is for delegates; the owner has no grant"))?;
    let s = ctx.scope();
    Ok(WhoAmI { grant: g, from_ms: (s.from_ms != i64::MIN).then_some(s.from_ms), to_ms: (s.to_ms != i64::MAX).then_some(s.to_ms) })
}

/// The grant's stable writer key for a delegate (grant ids restart in a new access.db), "owner" for the person.
fn writer(ctx: &Ctx) -> String {
    ctx.grant().map_or("owner".into(), |g| g.writer.clone())
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Propose {
    pub rows: Vec<ObsRow>,
    /// Why, for the person reviewing it
    #[serde(default)]
    pub note: Option<String>,
}

fn review_propose(ctx: &mut Ctx, r: Propose) -> Result<Proposed> {
    review::propose(ctx.db(), &ctx.scope(), &writer(ctx), &r.rows, r.note.as_deref(), ctx.now_ms)
}

fn derived_write(ctx: &mut Ctx, r: Derive) -> Result<Derived> {
    let mut out = health_core::derived::write_derived(ctx.db(), &ctx.scope(), &writer(ctx), &r, ctx.now_ms)?;
    // a daily-only grant could narrow the range to learn exact reading times from the count
    if ctx.grant().is_some_and(|g| !g.spec.detail) {
        out.inputs = None;
    }
    Ok(out)
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PathReq {
    pub path: String,
}

fn import_apple(ctx: &mut Ctx, r: PathReq) -> Result<FileImported> {
    health_core::import::apple(ctx.db(), r.path.as_ref(), ctx.now_ms)
}

fn import_whoop(ctx: &mut Ctx, r: PathReq) -> Result<FileImported> {
    let text = std::fs::read_to_string(&r.path).map_err(|_| Error::not_found("no_file", r.path.clone()))?;
    let bundle: health_core::whoop::WhoopData = serde_json::from_str(&text).map_err(|e| Error::invalid("bad_whoop", e.to_string()))?;
    health_core::import::whoop(ctx.db(), &bundle, ctx.now_ms)
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RowsFile {
    pub path: String,
    /// Source name for every row, e.g. "Clinic export"
    pub source: String,
}

fn import_rows(ctx: &mut Ctx, r: RowsFile) -> Result<FileImported> {
    health_core::import::rows(ctx.db(), r.path.as_ref(), &r.source, ctx.now_ms)
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExportGroups {
    #[serde(default)]
    pub from_ms: Option<i64>,
}

fn export_groups(ctx: &mut Ctx, r: ExportGroups) -> Result<Vec<GroupCount>> {
    health_core::csv::groups(ctx.db(), r.from_ms.unwrap_or(i64::MIN))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExportCsv {
    /// Output file (must not exist)
    pub path: String,
    #[serde(default)]
    pub from_ms: Option<i64>,
    /// Groups to include; sensitive ones only when named
    pub groups: Vec<String>,
    /// Keep dates only (no time or time zone)
    #[serde(default)]
    pub date_only: bool,
}

#[derive(Serialize, JsonSchema)]
pub struct ExportedCsv {
    pub path: String,
    pub rows: usize,
}

fn export_csv(ctx: &mut Ctx, r: ExportCsv) -> Result<ExportedCsv> {
    let p = std::path::Path::new(&r.path);
    if p.exists() {
        return Err(Error::conflict("exists", r.path.clone()));
    }
    let mut f = std::io::BufWriter::new(health_core::profile::create_private(p)?);
    let res = health_core::csv::write(ctx.db(), r.from_ms.unwrap_or(i64::MIN), &r.groups, r.date_only, &mut f)
        .and_then(|rows| std::io::Write::flush(&mut f).map(|_| rows).map_err(Into::into));
    match res {
        Ok(rows) => Ok(ExportedCsv { path: r.path, rows }),
        Err(e) => {
            drop(f);
            let _ = std::fs::remove_file(p); // no half-written file left behind
            Err(e)
        }
    }
}
