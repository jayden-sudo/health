//! `health` CLI: parses arguments into an operation request and calls health_api. No rules live here.

mod client;
mod keys;
mod mcp;
mod render;
mod serve;
mod time;

use clap::{Args, Parser, Subcommand};
use health_api::{Caller, Ctx};
use health_core::keys::KeyStore;
use health_core::profile::Profile;
use health_core::{Error, Kind, Result};
use serde_json::{Value, json};
use std::io::Read;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "health", version, about = "Private, local-first personal health data")]
struct Cli {
    /// Data directory (default: the platform's app data dir)
    #[arg(long, global = true, env = "HEALTH_DATA_DIR")]
    data_dir: Option<PathBuf>,
    #[arg(long, global = true, env = "HEALTH_PROFILE", default_value = "default")]
    profile: String,
    /// Print the operation's response (or error) as JSON
    #[arg(long, global = true)]
    json: bool,
    /// Run a write and roll it back, reporting what would change
    #[arg(long, global = true)]
    dry_run: bool,
    /// Passphrase-locked profile: read the passphrase from stdin's first line
    #[arg(long, global = true)]
    passphrase_stdin: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create the profile (new key in the keychain, empty encrypted database)
    Init,
    /// Profile and database summary
    Status,
    /// List operations, or print one operation's request and response JSON Schema
    Schema { op: Option<String> },
    /// Call any operation with a JSON request ("-" reads stdin)
    Op {
        name: String,
        #[arg(default_value = "{}")]
        request: String,
    },
    #[command(subcommand)]
    Metric(MetricCmd),
    /// Enter a value by hand. Multi-unit metrics need --unit
    Add(AddArgs),
    /// Replace a manual entry (identified by metric and time)
    Edit {
        metric: String,
        /// Time of the entry to replace (as entered)
        at: String,
        /// New value (omit for a span metric)
        #[arg(allow_negative_numbers = true)]
        value: Option<String>,
        #[arg(long)]
        unit: Option<String>,
        /// Move the entry to this time (default: keep it where it is)
        #[arg(long)]
        new_at: Option<String>,
        #[arg(long)]
        end: Option<String>,
    },
    /// Delete a manual entry
    Rm { metric: String, at: String },
    #[command(subcommand)]
    Obs(ObsCmd),
    /// Daily summaries of a metric, one elected source per day
    Daily {
        metric: Option<String>,
        #[arg(long)]
        from: Option<String>,
        #[arg(long)]
        to: Option<String>,
        /// Rebuild every daily summary instead
        #[arg(long, conflicts_with = "metric")]
        rebuild: bool,
    },
    /// Correct (or --delete) a point from a device or lab; values are in the metric's canonical unit
    Correct {
        metric: String,
        source: i64,
        at: String,
        #[arg(required_unless_present = "delete", allow_negative_numbers = true)]
        value: Option<String>,
        #[arg(long)]
        delete: bool,
    },
    #[command(subcommand)]
    Source(SourceCmd),
    #[command(subcommand)]
    Import(ImportCmd),
    #[command(subcommand)]
    Review(ReviewCmd),
    #[command(subcommand)]
    Report(ReportCmd),
    #[command(subcommand)]
    Backup(BackupCmd),
    /// Passphrase lock: keep the root key out of the keychain (plan §3.3)
    #[command(subcommand)]
    Passphrase(PassphraseCmd),
    /// Run the single writer on 127.0.0.1 (for the GUI, agents and forwarded CLI calls)
    Serve {
        /// Fixed port (default: random)
        #[arg(long, default_value_t = 0)]
        port: u16,
        /// Passphrase mode: lock again after this many idle minutes
        #[arg(long, default_value_t = 15)]
        idle_lock: u64,
        /// Exit when stdin closes, i.e. when the process that started this one exits (the GUI shell uses it).
        /// Not for launchd, nohup or `</dev/null`: a closed stdin means exit at once
        #[arg(long)]
        exit_with_parent: bool,
    },
    /// MCP over stdio for an agent; needs HEALTH_TOKEN or HEALTH_TOKEN_FILE (a grant token)
    Mcp,
    /// Unlock a running serve (passphrase mode)
    Unlock,
    /// Lock a running serve now (passphrase mode)
    Lock,
    /// Grants for agents
    #[command(subcommand)]
    Grant(GrantCmd),
    /// Unencrypted export for external analysis
    #[command(subcommand)]
    Export(ExportCmd),
    /// Who read what, when (no values)
    Audit {
        #[arg(long)]
        grant: Option<i64>,
        #[arg(long, default_value_t = 50)]
        limit: i64,
    },
}

#[derive(Subcommand)]
enum ExportCmd {
    /// Rows per data group
    Groups {
        #[arg(long)]
        from: Option<String>,
    },
    /// Write a CSV of the chosen groups (default: the normal ones)
    Csv {
        path: PathBuf,
        #[arg(long)]
        from: Option<String>,
        #[arg(long, value_delimiter = ',', default_value = "body,sleep,cardio,activity,lab")]
        groups: Vec<String>,
        #[arg(long)]
        date_only: bool,
    },
}

#[derive(Subcommand)]
enum GrantCmd {
    /// Create a grant; prints its token once
    Create {
        /// Shown to you, e.g. "Claude Desktop"
        #[arg(long)]
        name: String,
        /// Comma-separated: body,sleep,cardio,activity,lab (normal); infectious,tumor,repro,custom,lab_other,other,derived (sensitive)
        #[arg(long, value_delimiter = ',')]
        groups: Vec<String>,
        /// Allow individual points (default: daily summaries only)
        #[arg(long)]
        detail: bool,
        /// Allow full report text (not limited by groups: a lab sheet shows every item on it)
        #[arg(long)]
        reports: bool,
        /// Only the last N days (default: everything)
        #[arg(long)]
        days: Option<i64>,
        /// none | propose | derived
        #[arg(long, default_value = "none")]
        write: String,
        /// Expires after N days
        #[arg(long, default_value_t = 30)]
        expires_days: i64,
    },
    List,
    Revoke {
        id: i64,
    },
}

#[derive(Subcommand)]
enum PassphraseCmd {
    /// Wrap the root key with a passphrase and delete it from the keychain
    Set,
    /// Move the root key back into the keychain
    Remove,
}

#[derive(Args)]
struct AddArgs {
    metric: String,
    /// The value; omit for a span metric (e.g. deep sleep with --end)
    #[arg(allow_negative_numbers = true)]
    value: Option<String>,
    #[arg(long)]
    unit: Option<String>,
    /// YYYY-MM-DD (date only), local YYYY-MM-DDTHH:MM, RFC 3339, or epoch ms (default: now)
    #[arg(long)]
    at: Option<String>,
    /// End of a span (e.g. a sleep period)
    #[arg(long)]
    end: Option<String>,
}

#[derive(Subcommand)]
enum MetricCmd {
    Search {
        q: String,
        #[arg(long, default_value_t = 20)]
        limit: i64,
    },
    Show {
        code: String,
    },
}

#[derive(Subcommand)]
enum ObsCmd {
    Query {
        metric: String,
        #[arg(long)]
        from: Option<String>,
        #[arg(long)]
        to: Option<String>,
        #[arg(long)]
        source: Option<i64>,
        #[arg(long, default_value_t = 1000)]
        limit: i64,
    },
    Latest {
        metric: Option<String>,
    },
}

#[derive(Subcommand)]
enum SourceCmd {
    List,
    Rename {
        id: i64,
        name: String,
    },
    /// Smaller wins the daily election
    Priority {
        id: i64,
        priority: i64,
    },
}

#[derive(Subcommand)]
enum ImportCmd {
    /// Apple Health "Export All Health Data" zip
    Apple {
        path: PathBuf,
    },
    /// A saved WHOOP API bundle (JSON)
    Whoop {
        path: PathBuf,
    },
    /// JSON lines of rows from one source
    Rows {
        path: PathBuf,
        #[arg(long)]
        source: String,
    },
    List {
        #[arg(long)]
        connector: Option<String>,
    },
    Show {
        id: i64,
        #[arg(long, default_value_t = 50)]
        limit: i64,
    },
    Erase {
        id: i64,
    },
}

#[derive(Subcommand)]
enum ReviewCmd {
    List,
    /// Confirm: an agent's proposal as proposed, or with a JSON array of rows (file or "-")
    Confirm {
        id: i64,
        rows: Option<String>,
        #[arg(long)]
        text: Option<String>,
    },
    Discard {
        id: i64,
    },
}

#[derive(Subcommand)]
enum ReportCmd {
    Search {
        q: String,
        #[arg(long, default_value_t = 20)]
        limit: i64,
    },
    Show {
        id: i64,
    },
}

#[derive(Subcommand)]
enum BackupCmd {
    Export {
        path: PathBuf,
        #[arg(long)]
        no_blobs: bool,
        /// Read the password from stdin instead of prompting
        #[arg(long)]
        password_stdin: bool,
    },
    Import {
        path: PathBuf,
        #[arg(long)]
        password_stdin: bool,
    },
}

fn usage(code: &'static str, msg: impl Into<String>) -> Error {
    Error::new(Kind::Usage, code, msg)
}

fn read_arg(s: &str) -> Result<String> {
    if s == "-" {
        let mut b = String::new();
        std::io::stdin().read_to_string(&mut b)?;
        Ok(b)
    } else if s.trim_start().starts_with(['{', '[']) {
        Ok(s.to_string())
    } else {
        std::fs::read_to_string(s).map_err(|_| usage("bad_arg", format!("{s}: not JSON and not a readable file")))
    }
}

/// Integer, finite number, or text (never NaN/inf: JSON would turn those into null, which deletes).
fn value_of(s: &str) -> Value {
    if let Ok(i) = s.parse::<i64>() {
        return Value::from(i);
    }
    match s.parse::<f64>() {
        Ok(f) if f.is_finite() => Value::from(f),
        _ => Value::from(s),
    }
}

fn entry(metric: &str, value: Option<&str>, unit: Option<&str>, at: &str, end: Option<&str>) -> Result<Value> {
    let at = time::parse(at)?;
    let at_json = match (&at.date, end) {
        (Some(d), None) => json!({ "date": d, "tz_min": at.tz_min }),
        (_, end) => {
            let end = end.map(time::parse).transpose()?.map(|e| e.ms);
            json!({ "ms": at.ms, "end_ms": end, "tz_min": at.tz_min })
        }
    };
    let mut e = json!({ "metric": metric, "value": value.map_or(Value::Null, value_of), "at": at_json });
    if let Some(u) = unit {
        e["unit"] = json!(u);
    }
    Ok(e)
}

fn password(from_stdin: bool, confirm: bool) -> Result<String> {
    if from_stdin {
        let mut s = String::new();
        std::io::stdin().read_line(&mut s)?;
        return Ok(s.trim_end_matches(['\r', '\n']).to_string());
    }
    let p = rpassword::prompt_password("Backup password: ")?;
    if confirm && rpassword::prompt_password("Repeat: ")? != p {
        return Err(usage("password_mismatch", "passwords differ"));
    }
    Ok(p)
}

/// Turns a command into (operation, request).
fn request(cmd: &Cmd, now: i64) -> Result<(String, Value)> {
    let day = |s: &Option<String>| -> Result<Option<i64>> { s.as_deref().map(|s| time::parse(s).map(|w| w.ms)).transpose() };
    Ok(match cmd {
        Cmd::Init | Cmd::Schema { .. } | Cmd::Passphrase(_) | Cmd::Serve { .. } | Cmd::Mcp | Cmd::Unlock | Cmd::Lock => {
            unreachable!("handled before")
        }
        Cmd::Grant(GrantCmd::Create { name, groups, detail, reports, days, write, expires_days }) => (
            "grant.create".into(),
            json!({
                "name": name, "groups": groups, "detail": detail, "reports": reports,
                "window": days.map_or(json!("all"), |d| json!({ "last_days": d })),
                "write": write,
                "expires_ms": expires_days.checked_mul(86_400_000).and_then(|d| now.checked_add(d)).ok_or_else(|| usage("bad_expiry", "too far ahead"))?,
            }),
        ),
        Cmd::Grant(GrantCmd::List) => ("grant.list".into(), json!({})),
        Cmd::Grant(GrantCmd::Revoke { id }) => ("grant.revoke".into(), json!({ "id": id })),
        Cmd::Audit { grant, limit } => ("audit.list".into(), json!({ "grant_id": grant, "limit": limit })),
        Cmd::Status => ("status".into(), json!({})),
        Cmd::Op { name, request } => {
            let mut req: Value = serde_json::from_str(&read_arg(request)?).map_err(|e| usage("bad_json", e.to_string()))?;
            // absolute, as the typed commands do: a forwarded call runs in serve's working directory
            if let Some(p) = req.get("path").and_then(Value::as_str).filter(|p| !p.is_empty()).map(std::path::PathBuf::from) {
                req["path"] = json!(std::path::absolute(p)?);
            }
            (name.clone(), req)
        }
        Cmd::Metric(MetricCmd::Search { q, limit }) => ("metric.search".into(), json!({ "q": q, "limit": limit })),
        Cmd::Metric(MetricCmd::Show { code }) => ("metric.show".into(), json!({ "code": code })),
        Cmd::Add(a) => {
            let at = a.at.clone().unwrap_or_else(|| now.to_string());
            let e = entry(&a.metric, a.value.as_deref(), a.unit.as_deref(), &at, a.end.as_deref())?;
            ("manual.enter".into(), json!({ "entries": [e] }))
        }
        Cmd::Edit { metric, at, value, unit, new_at, end } => {
            let e = entry(metric, value.as_deref(), unit.as_deref(), new_at.as_deref().unwrap_or(at), end.as_deref())?;
            ("manual.edit".into(), json!({ "code": metric, "start_ms": time::parse(at)?.ms, "entry": e }))
        }
        Cmd::Rm { metric, at } => ("manual.remove".into(), json!({ "code": metric, "start_ms": time::parse(at)?.ms })),
        Cmd::Obs(ObsCmd::Query { metric, from, to, source, limit }) => {
            ("obs.query".into(), json!({ "metric": metric, "from_ms": day(from)?, "to_ms": day(to)?, "source_id": source, "limit": limit }))
        }
        Cmd::Obs(ObsCmd::Latest { metric }) => ("obs.latest".into(), json!({ "metric": metric })),
        Cmd::Daily { rebuild: true, .. } => ("daily.rebuild".into(), json!({})),
        Cmd::Daily { metric: None, .. } => return Err(usage("bad_arg", "give a metric, or --rebuild")),
        Cmd::Daily { metric: Some(m), from, to, .. } => ("daily.query".into(), json!({ "metric": m, "from": from, "to": to })),
        Cmd::Correct { metric, source, at, value, delete } => {
            let v = if *delete { Value::Null } else { value_of(value.as_deref().unwrap_or_default()) };
            ("correct".into(), json!({ "metric": metric, "source_id": source, "start_ms": time::parse(at)?.ms, "value": v }))
        }
        Cmd::Source(SourceCmd::List) => ("source.list".into(), json!({})),
        Cmd::Source(SourceCmd::Rename { id, name }) => ("source.update".into(), json!({ "id": id, "name": name })),
        Cmd::Source(SourceCmd::Priority { id, priority }) => ("source.update".into(), json!({ "id": id, "priority": priority })),
        Cmd::Import(ImportCmd::Apple { path }) => ("import.apple".into(), json!({ "path": std::path::absolute(path)? })),
        Cmd::Import(ImportCmd::Whoop { path }) => ("import.whoop".into(), json!({ "path": std::path::absolute(path)? })),
        Cmd::Import(ImportCmd::Rows { path, source }) => {
            ("import.rows".into(), json!({ "path": std::path::absolute(path)?, "source": source }))
        }
        Cmd::Export(ExportCmd::Groups { from }) => ("export.groups".into(), json!({ "from_ms": day(from)? })),
        Cmd::Export(ExportCmd::Csv { path, from, groups, date_only }) => (
            "export.csv".into(),
            json!({ "path": std::path::absolute(path)?, "from_ms": day(from)?, "groups": groups, "date_only": date_only }),
        ),
        Cmd::Import(ImportCmd::List { connector }) => ("import.list".into(), json!({ "connector": connector })),
        Cmd::Import(ImportCmd::Show { id, limit }) => ("import.show".into(), json!({ "id": id, "limit": limit })),
        Cmd::Import(ImportCmd::Erase { id }) => ("import.erase".into(), json!({ "id": id })),
        Cmd::Review(ReviewCmd::List) => ("review.list".into(), json!({})),
        Cmd::Review(ReviewCmd::Confirm { id, rows, text }) => {
            let rows: Value = match rows {
                Some(r) => serde_json::from_str(&read_arg(r)?).map_err(|e| usage("bad_json", e.to_string()))?,
                None => Value::Null,
            };
            ("review.confirm".into(), json!({ "id": id, "rows": rows, "text": text }))
        }
        Cmd::Review(ReviewCmd::Discard { id }) => ("review.discard".into(), json!({ "id": id })),
        Cmd::Report(ReportCmd::Search { q, limit }) => ("report.search".into(), json!({ "q": q, "limit": limit })),
        Cmd::Report(ReportCmd::Show { id }) => ("report.show".into(), json!({ "id": id })),
        // absolute: a forwarded call runs in serve's working directory
        Cmd::Backup(BackupCmd::Export { path, no_blobs, password_stdin }) => (
            "backup.export".into(),
            json!({ "path": std::path::absolute(path)?, "no_blobs": no_blobs, "password": password(*password_stdin, true)? }),
        ),
        Cmd::Backup(BackupCmd::Import { path, password_stdin }) => {
            ("backup.import".into(), json!({ "path": std::path::absolute(path)?, "password": password(*password_stdin, false)? }))
        }
    })
}

fn data_dir(cli: &Cli) -> Result<PathBuf> {
    cli.data_dir.clone().or_else(|| dirs::data_dir().map(|d| d.join("health"))).ok_or_else(|| usage("no_data_dir", "set --data-dir"))
}

fn passphrase_cmd(cli: &Cli, dir: &std::path::Path, cmd: &PassphraseCmd) -> Result<Value> {
    let pdir = health_core::profile::profile_dir(dir, &cli.profile)?;
    let id = std::fs::read_to_string(pdir.join("id")).map_err(|_| Error::not_found("no_profile", cli.profile.clone()))?;
    let id = id.trim();
    let file = pdir.join(health_core::keys::KEYWRAP);
    // a running serve fixed its mode at start: it would keep the profile open, or lose its key file
    if matches!(cmd, PassphraseCmd::Set | PassphraseCmd::Remove) && client::probe(&pdir).is_some() {
        return Err(Error::conflict("serve_running", "stop `health serve` (quit the app) first, then try again"));
    }
    match cmd {
        PassphraseCmd::Set => {
            if file.exists() {
                return Err(Error::conflict("exists", "a passphrase is already set"));
            }
            let current = keys::from_env();
            let root = current.load(id)?.ok_or_else(|| Error::new(Kind::Locked, "no_key", "no key to wrap"))?;
            let p = keys::read_passphrase(cli.passphrase_stdin, "New passphrase: ")?;
            if !cli.passphrase_stdin && keys::read_passphrase(false, "Repeat: ")? != p {
                return Err(usage("passphrase_mismatch", "passphrases differ"));
            }
            let store = health_core::keys::PassphraseStore::new(&file, &p);
            store.store(id, &root)?;
            if store.load(id)?.as_deref() != Some(&*root) {
                let _ = std::fs::remove_file(&file);
                return Err(Error::other("keywrap", "verification failed; nothing changed"));
            }
            let note = if current.kind() == "keychain" {
                keys::keychain_delete(id)?;
                "root key removed from the keychain".to_string()
            } else {
                "the key file still holds the root key: delete it yourself once you have checked the passphrase".to_string()
            };
            Ok(json!({ "key_store": "passphrase", "note": note }))
        }
        PassphraseCmd::Remove => {
            let p = keys::read_passphrase(cli.passphrase_stdin, "Passphrase: ")?;
            let root = health_core::keys::PassphraseStore::new(&file, &p).load(id)?.ok_or_else(|| Error::not_found("no_passphrase", ""))?;
            let target = keys::from_env();
            if target.load(id)?.as_deref() != Some(&*root) {
                target.store(id, &root)?;
            }
            std::fs::remove_file(&file)?;
            Ok(json!({ "key_store": target.kind() }))
        }
    }
}

/// How the owner proves itself to a running serve: its token (keychain mode) or the passphrase proof.
fn owner_auth(cli: &Cli, pdir: &std::path::Path, s: &client::Serve) -> Result<(&'static str, String)> {
    if let Some(t) = &s.token {
        return Ok(("Authorization", format!("Bearer {t}")));
    }
    let p = keys::read_passphrase(cli.passphrase_stdin, "Passphrase: ")?;
    Ok(("X-Health-Proof", health_core::keys::PassphraseStore::new(&pdir.join(health_core::keys::KEYWRAP), &p).proof()?))
}

/// The grant token for `health mcp`: HEALTH_TOKEN, or HEALTH_TOKEN_FILE readable by its owner only.
fn agent_token() -> Result<String> {
    if let Ok(t) = std::env::var("HEALTH_TOKEN") {
        return Ok(t.trim().to_string());
    }
    let f = std::env::var_os("HEALTH_TOKEN_FILE")
        .ok_or_else(|| Error::new(Kind::Denied, "no_token", "set HEALTH_TOKEN or HEALTH_TOKEN_FILE"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if std::fs::metadata(&f)?.permissions().mode() & 0o077 != 0 {
            return Err(Error::new(Kind::Denied, "no_token", "HEALTH_TOKEN_FILE must be mode 0600"));
        }
    }
    Ok(std::fs::read_to_string(f)?.trim().to_string())
}

/// MCP over stdio: one JSON-RPC message per line; stdout carries nothing else.
fn mcp_stdio(dir: &std::path::Path, profile: &str) -> Result<()> {
    use std::io::{BufRead, Write};
    let token = agent_token()?;
    let pdir = health_core::profile::profile_dir(dir, profile)?;
    let mut run = |op: &str, args: Value| -> Result<health_api::Outcome> {
        if let Some(s) = client::probe(&pdir) {
            let (value, withheld) = s.op(("Authorization", format!("Bearer {token}")), op, &args, false)?;
            return Ok(health_api::Outcome { value, withheld: withheld.and_then(|w| serde_json::from_value(w).ok()) });
        }
        if pdir.join(health_core::keys::KEYWRAP).exists() {
            return Err(Error::new(Kind::Locked, "locked", "passphrase-locked: start and unlock `health serve`"));
        }
        let now = time::now_ms();
        let write = health_api::find(op)?.write;
        let p = Profile::open(dir, profile, keys::from_env().as_ref(), write, now)?;
        let grant = p.access.resolve(&token, now)?;
        let mut ctx = Ctx::new(&p, now, Caller::Delegate(grant));
        health_api::call(&mut ctx, op, args)
    };
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(msg) => mcp::handle(&msg, &mut run),
            Err(_) => Some(json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": "parse error" } })),
        };
        if let Some(r) = reply {
            writeln!(out, "{r}")?;
            out.flush()?;
        }
    }
    Ok(())
}

fn run(cli: &Cli) -> Result<Value> {
    let now = time::now_ms();
    let dir = data_dir(cli)?;
    let pdir = health_core::profile::profile_dir(&dir, &cli.profile)?;
    match &cli.cmd {
        Cmd::Passphrase(cmd) => return passphrase_cmd(cli, &dir, cmd),
        Cmd::Serve { port, idle_lock, exit_with_parent } => {
            if *exit_with_parent {
                // The shell holds our stdin pipe; when it exits (or crashes) the pipe closes. Works on every OS.
                std::thread::spawn(|| {
                    let _ = std::io::copy(&mut std::io::stdin(), &mut std::io::sink());
                    std::process::exit(0);
                });
            }
            serve::run(serve::Opts {
                data_dir: dir,
                profile: cli.profile.clone(),
                port: *port,
                idle_lock: std::time::Duration::from_secs((*idle_lock).max(1) * 60),
            })?;
            return Ok(Value::Null);
        }
        Cmd::Mcp => {
            mcp_stdio(&dir, &cli.profile)?;
            return Ok(Value::Null);
        }
        Cmd::Unlock | Cmd::Lock => {
            let s = client::probe(&pdir).ok_or_else(|| Error::not_found("no_serve", "no serve is running for this profile"))?;
            let (path, body, headers) = if matches!(cli.cmd, Cmd::Unlock) {
                let p = keys::read_passphrase(cli.passphrase_stdin, "Passphrase: ")?;
                ("/v1/unlock", json!({ "passphrase": p }).to_string(), vec![])
            } else {
                ("/v1/lock", String::new(), vec![owner_auth(cli, &pdir, &s)?])
            };
            let (status, b) = s.call("POST", path, &headers, body.as_bytes())?;
            let v: Value = serde_json::from_slice(&b).unwrap_or(Value::Null);
            if status != 200 {
                let code = v["error"]["code"].as_str().unwrap_or("serve").to_string();
                return Err(Error::new(
                    client::kind_of(status),
                    Box::leak(code.into_boxed_str()),
                    v["error"]["message"].as_str().unwrap_or("").to_string(),
                ));
            }
            return Ok(v);
        }
        Cmd::Init | Cmd::Schema { .. } => {}
        _ => {
            if let Some(s) = client::probe(&pdir) {
                // the passphrase first, as on the direct path, so stdin lines mean the same either way
                let auth = owner_auth(cli, &pdir, &s)?;
                let (name, req) = request(&cli.cmd, now)?;
                return Ok(s.op(auth, &name, &req, cli.dry_run)?.0);
            }
        }
    }
    let keys = match &cli.cmd {
        Cmd::Init | Cmd::Schema { .. } => keys::from_env(),
        _ => keys::for_profile(&health_core::profile::profile_dir(&dir, &cli.profile)?, cli.passphrase_stdin)?,
    };
    match &cli.cmd {
        Cmd::Init if cli.dry_run => return Err(usage("no_dry_run", "init")),
        Cmd::Init => {
            let p = Profile::init(&dir, &cli.profile, keys.as_ref())?;
            return Ok(json!({ "profile": p.name, "dir": p.dir, "key_store": p.key_store }));
        }
        Cmd::Schema { op: None } => {
            return Ok(Value::Array(
                health_api::ops()
                    .iter()
                    .map(|o| json!({ "op": o.name, "write": o.write, "delegate": format!("{:?}", o.delegate).to_lowercase(), "about": o.about }))
                    .collect(),
            ));
        }
        Cmd::Schema { op: Some(name) } => {
            let o = health_api::find(name)?;
            return Ok(json!({ "op": o.name, "about": o.about, "request": (o.req)(), "response": (o.resp)() }));
        }
        _ => {}
    }
    let (name, req) = request(&cli.cmd, now)?;
    let op = health_api::find(&name)?;
    let profile = Profile::open(&dir, &cli.profile, keys.as_ref(), op.write, now)?;
    let mut ctx = Ctx::new(&profile, now, Caller::Owner);
    ctx.dry_run = cli.dry_run;
    Ok(health_api::call(&mut ctx, &name, req)?.value)
}

fn main() {
    #[cfg(unix)]
    // Every file this process creates is private (database, WAL, blobs, backups).
    unsafe {
        libc::umask(0o077);
    }
    let cli = Cli::parse();
    match run(&cli) {
        Ok(Value::Null) => {} // serve / mcp: stdout is theirs
        Ok(v) => {
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&v).expect("json"));
            } else {
                render::print(&v);
            }
        }
        Err(e) => {
            if cli.json {
                println!("{}", json!({ "error": { "code": e.code, "message": e.detail } }));
            } else {
                eprintln!("error: {e}");
            }
            std::process::exit(e.kind.exit_code());
        }
    }
}
