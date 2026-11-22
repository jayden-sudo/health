//! CLI contract: --json shapes, exit codes, backup round trip. Each test uses its own data dir and key file.

use serde_json::Value;
use std::io::Write;
use std::process::{Command, Stdio};

struct Env {
    dir: tempfile::TempDir,
}

impl Env {
    fn new() -> Env {
        let e = Env { dir: tempfile::tempdir().unwrap() };
        assert_eq!(e.run(&["init"]).0, 0);
        e
    }
    fn cmd(&self, profile: &str) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_health"));
        c.env("HEALTH_DATA_DIR", self.dir.path().join("data"))
            .env("HEALTH_ROOT_KEY_FILE", self.dir.path().join(format!("{profile}.key")))
            .env("HEALTH_PROFILE", profile)
            .env("HEALTH_NOW", "1795176000000")
            .env("HEALTH_TZ_MIN", "480")
            .arg("--json");
        c
    }
    fn run_in(&self, profile: &str, args: &[&str], stdin: &str) -> (i32, Value) {
        let mut child = self.cmd(profile).args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
        child.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
        let out = child.wait_with_output().unwrap();
        let text = String::from_utf8(out.stdout).unwrap();
        (out.status.code().unwrap(), serde_json::from_str(&text).unwrap_or(Value::String(text)))
    }
    fn run(&self, args: &[&str]) -> (i32, Value) {
        self.run_in("default", args, "")
    }
}

#[test]
fn enter_query_and_remove() {
    let e = Env::new();
    let (code, v) = e.run(&["add", "loinc:2345-7", "5.5", "--unit", "mmol/L", "--at", "2026-11-11T07:00"]);
    assert_eq!((code, v["written"].as_u64()), (0, Some(1)));
    let (_, v) = e.run(&["obs", "latest", "loinc:2345-7"]);
    assert!((v[0]["value"].as_f64().unwrap() - 99.0858).abs() < 1e-9);
    assert_eq!(v[0]["tz_min"], 480);
    let (_, d) = e.run(&["daily", "loinc:2345-7"]);
    assert_eq!(d[0]["day"], "2026-11-11");
    assert_eq!(e.run(&["rm", "loinc:2345-7", "2026-11-11T07:00"]).0, 0);
    assert_eq!(e.run(&["obs", "latest"]).1, Value::Array(vec![]));
}

#[test]
fn exit_codes_follow_the_contract() {
    let e = Env::new();
    let (code, v) = e.run(&["add", "loinc:2345-7", "5.5"]);
    assert_eq!((code, v["error"]["code"].as_str()), (3, Some("unit_required")));
    assert_eq!(e.run(&["op", "no.such.op"]).0, 2);
    assert_eq!(e.run(&["metric", "show", "x:nope"]).0, 6);
    assert_eq!(e.run(&["op", "metric.search", r#"{"q":"a","bogus":1}"#]).0, 3);
    assert_eq!(e.run_in("other", &["status"], "").0, 6); // no such profile
    assert_eq!(e.run(&["init"]).0, 7); // exists
}

#[test]
fn dry_run_changes_nothing() {
    let e = Env::new();
    assert_eq!(e.run(&["--dry-run", "add", "loinc:8867-4", "60"]).0, 0);
    assert_eq!(e.run(&["status"]).1["observations"], 0);
}

#[test]
fn schema_lists_every_op() {
    let e = Env::new();
    let (_, ops) = e.run(&["schema"]);
    assert!(ops.as_array().unwrap().len() >= 20);
    let (_, s) = e.run(&["schema", "obs.query"]);
    assert!(s["request"]["properties"]["metric"].is_object());
}

#[test]
fn backup_round_trip_between_profiles() {
    let e = Env::new();
    e.run(&["add", "loinc:29463-7", "70.5", "--unit", "kg", "--at", "2026-11-10"]);
    let file = e.dir.path().join("b.zuhealth");
    let f = file.to_str().unwrap();
    assert_eq!(e.run_in("default", &["backup", "export", f, "--password-stdin"], "short\n").0, 3);
    let (code, v) = e.run_in("default", &["backup", "export", f, "--password-stdin"], "correct horse\n");
    assert_eq!(code, 0, "{v}");
    assert_eq!(e.run_in("other", &["init"], "").0, 0);
    assert_eq!(e.run_in("other", &["backup", "import", f, "--password-stdin"], "wrong pass\n").1["error"]["code"], "pack_auth");
    let (code, v) = e.run_in("other", &["backup", "import", f, "--password-stdin"], "correct horse\n");
    assert_eq!((code, v["added_imports"].as_u64()), (0, Some(1)), "{v}");
    assert_eq!(e.run_in("other", &["daily", "loinc:29463-7"], "").1[0]["sum"], 70.5);
}

#[test]
fn edit_keeps_the_time_unless_moved() {
    let e = Env::new();
    e.run(&["add", "loinc:29463-7", "70", "--unit", "kg", "--at", "2026-11-10"]);
    let (code, v) = e.run(&["edit", "loinc:29463-7", "2026-11-10", "71", "--unit", "kg"]);
    assert_eq!(code, 0, "{v}");
    let (_, p) = e.run(&["obs", "latest", "loinc:29463-7"]);
    assert_eq!((p[0]["value"].as_i64(), p[0]["detail"]["precision"].as_str()), (Some(71), Some("day")));
    assert_eq!(e.run(&["edit", "loinc:29463-7", "2026-11-10", "72", "--unit", "kg", "--new-at", "2026-11-12"]).0, 0);
    let (_, d) = e.run(&["daily", "loinc:29463-7"]);
    assert_eq!((d.as_array().unwrap().len(), d[0]["day"].as_str()), (1, Some("2026-11-12")));
}

#[test]
fn spans_rejections_and_corrections() {
    let e = Env::new();
    let (code, v) = e.run(&["add", "loinc:93831-6", "--at", "2026-11-10T23:00", "--end", "2026-11-11T01:30"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(e.run(&["daily", "loinc:93831-6"]).1[0]["seconds"], 9000);
    // a refused value saves nothing and says so
    assert_eq!(e.run(&["add", "loinc:8867-4", "fast"]).1["error"]["code"], "rejected");
    assert_eq!(e.run(&["import", "list"]).1.as_array().unwrap().len(), 1);
    // manual points are edited, not corrected
    let (_, p) = e.run(&["obs", "latest", "loinc:93831-6"]);
    let at = p[0]["start_ms"].to_string();
    let src = p[0]["source_id"].to_string();
    assert_eq!(e.run(&["correct", "loinc:93831-6", &src, &at, "--delete"]).1["error"]["code"], "use_edit");
}

#[test]
fn a_passphrase_lock_guards_every_command() {
    let e = Env::new();
    e.run(&["add", "loinc:8867-4", "60"]);
    assert_eq!(e.run_in("default", &["--passphrase-stdin", "passphrase", "set"], "a long passphrase\n").0, 0);
    // non-interactive without the passphrase: locked
    assert_eq!(e.run(&["status"]).0, 5);
    assert_eq!(e.run_in("default", &["--passphrase-stdin", "status"], "wrong one\n").1["error"]["code"], "bad_passphrase");
    let (code, v) = e.run_in("default", &["--passphrase-stdin", "status"], "a long passphrase\n");
    assert_eq!((code, v["observations"].as_i64(), v["key_store"].as_str()), (0, Some(1), Some("passphrase")));
    assert_eq!(e.run_in("default", &["--passphrase-stdin", "passphrase", "remove"], "a long passphrase\n").0, 0);
    assert_eq!(e.run(&["status"]).1["key_store"], "file");
}
