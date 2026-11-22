//! serve over real HTTP, CLI forwarding, SSE, passphrase mode, and MCP (plan M2 acceptance).

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};

struct Env {
    dir: tempfile::TempDir,
}

impl Env {
    fn new() -> Env {
        let e = Env { dir: tempfile::tempdir().unwrap() };
        assert_eq!(e.run(&["init"], "").0, 0);
        e
    }
    fn cmd(&self) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_health"));
        c.env("HEALTH_DATA_DIR", self.dir.path().join("data"))
            .env("HEALTH_ROOT_KEY_FILE", self.dir.path().join("root.key"))
            .env("HEALTH_NOW", "1795176000000")
            .env("HEALTH_TZ_MIN", "0")
            .env_remove("HEALTH_TOKEN");
        c
    }
    fn run(&self, args: &[&str], stdin: &str) -> (i32, Value) {
        let mut child = self.cmd().arg("--json").args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
        child.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
        let out = child.wait_with_output().unwrap();
        let text = String::from_utf8(out.stdout).unwrap();
        (out.status.code().unwrap(), serde_json::from_str(&text).unwrap_or(Value::String(text)))
    }
    /// Starts serve; returns it with its first stdout line ({port, token, id, locked}).
    fn serve(&self) -> (Kill, Value) {
        let mut c = self.cmd().args(["serve"]).stdout(Stdio::piped()).spawn().unwrap();
        let mut line = String::new();
        BufReader::new(c.stdout.take().unwrap()).read_line(&mut line).unwrap();
        (Kill(c), serde_json::from_str(&line).unwrap())
    }
    fn grant(&self, extra: &[&str]) -> String {
        let mut args = vec!["grant", "create", "--name", "agent", "--groups", "cardio,derived", "--days", "90", "--write", "derived"];
        args.extend_from_slice(extra);
        let (code, v) = self.run(&args, "");
        assert_eq!(code, 0, "{v}");
        v["token"].as_str().unwrap().to_string()
    }
    fn seed_rhr(&self) {
        for d in 1..=5 {
            let at = format!("2026-11-{:02}", 15 + d);
            assert_eq!(self.run(&["add", "loinc:40443-4", &(50 + d).to_string(), "--at", &at], "").0, 0);
        }
    }
}

struct Kill(Child);
impl Drop for Kill {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn http(port: u64, method: &str, path: &str, headers: &[(&str, &str)], body: &str) -> (u16, Value) {
    let mut s = TcpStream::connect(("127.0.0.1", port as u16)).unwrap();
    let mut req = format!("{method} {path} HTTP/1.1\r\nConnection: close\r\nContent-Length: {}\r\n", body.len());
    if !headers.iter().any(|(k, _)| *k == "Host") {
        req.push_str(&format!("Host: 127.0.0.1:{port}\r\n"));
    }
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    req.push_str(body);
    s.write_all(req.as_bytes()).unwrap();
    let mut raw = String::new();
    s.read_to_string(&mut raw).unwrap();
    let status = raw.split_whitespace().nth(1).unwrap().parse().unwrap();
    let body = raw.split("\r\n\r\n").nth(1).unwrap_or("");
    (status, serde_json::from_str(body).unwrap_or(Value::Null))
}

#[test]
fn requests_are_checked_in_order_origin_host_token() {
    let e = Env::new();
    e.seed_rhr();
    let token = e.grant(&[]);
    let (_s, info) = e.serve();
    let port = info["port"].as_u64().unwrap();
    let owner = format!("Bearer {}", info["token"].as_str().unwrap());
    let agent = format!("Bearer {token}");
    // a web page (any Origin) and DNS rebinding (foreign Host) are refused even with the owner token
    assert_eq!(http(port, "POST", "/v1/op/status", &[("Authorization", &owner), ("Origin", "https://evil.example")], "{}").0, 403);
    assert_eq!(http(port, "POST", "/v1/op/status", &[("Authorization", &owner), ("Host", "evil.example")], "{}").0, 403);
    assert_eq!(http(port, "POST", "/v1/op/status", &[], "{}").0, 403);
    assert_eq!(http(port, "POST", "/v1/op/status", &[("Authorization", &owner)], "{}").0, 200);
    // a grant token on an owner-only op, and on a read it may do
    assert_eq!(http(port, "POST", "/v1/op/backup.export", &[("Authorization", &agent)], "{}").0, 403);
    let (code, v) = http(port, "POST", "/v1/op/daily.query", &[("Authorization", &agent)], r#"{"metric":"loinc:40443-4"}"#);
    assert_eq!((code, v["value"].as_array().map(Vec::len)), (200, Some(5)), "{v}");
    assert!(v["withheld"].is_object());
    assert_eq!(http(port, "GET", "/v1/events", &[("Authorization", &agent)], "").0, 404);
    assert_eq!(http(port, "GET", "/v1/meta", &[], "").1["id"], info["id"]);
    // revoked: refused at once, over the same connection type
    let id = e.run(&["grant", "list"], "").1[0]["id"].as_i64().unwrap();
    assert_eq!(e.run(&["grant", "revoke", &id.to_string()], "").0, 0);
    assert_eq!(http(port, "POST", "/v1/op/daily.query", &[("Authorization", &agent)], r#"{"metric":"loinc:40443-4"}"#).0, 403);
}

#[test]
fn the_cli_forwards_to_a_running_serve_and_events_follow_writes() {
    let e = Env::new();
    let (_s, info) = e.serve();
    let port = info["port"].as_u64().unwrap();
    let mut events = TcpStream::connect(("127.0.0.1", port as u16)).unwrap();
    write!(
        events,
        "GET /v1/events HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {}\r\n\r\n",
        info["token"].as_str().unwrap()
    )
    .unwrap();
    // serve holds the writer lock, so this write only works by forwarding
    let (code, v) = e.run(&["add", "loinc:8867-4", "61"], "");
    assert_eq!(code, 0, "{v}");
    assert_eq!(e.run(&["status"], "").1["observations"], 1);
    let mut seen = String::new();
    events.set_read_timeout(Some(std::time::Duration::from_secs(10))).unwrap(); // fail, never hang
    let mut r = BufReader::new(events);
    while !seen.contains("manual.enter") {
        let mut l = String::new();
        assert!(r.read_line(&mut l).unwrap() > 0, "event stream closed: {seen}");
        seen.push_str(&l);
    }
    assert!(seen.contains("event: changed"));
}

#[test]
fn passphrase_mode_starts_locked_and_never_writes_the_owner_token() {
    let e = Env::new();
    assert_eq!(e.run(&["--passphrase-stdin", "passphrase", "set"], "a long passphrase\n").0, 0);
    let (_s, info) = e.serve();
    assert_eq!(info["locked"], true);
    let file = std::fs::read_to_string(e.dir.path().join("data/profiles/default/serve.json")).unwrap();
    assert!(!file.contains(info["token"].as_str().unwrap()));
    assert_eq!(e.run(&["--passphrase-stdin", "status"], "a long passphrase\n").0, 5); // locked
    assert_eq!(e.run(&["--passphrase-stdin", "unlock"], "wrong passphrase\n").0, 5);
    assert_eq!(e.run(&["--passphrase-stdin", "unlock"], "a long passphrase\n").0, 0);
    // already unlocked: a wrong passphrase must not replace the owner proof
    assert_eq!(e.run(&["--passphrase-stdin", "unlock"], "attacker passphrase\n").0, 5);
    assert_eq!(e.run(&["--passphrase-stdin", "status"], "attacker passphrase\n").0, 4);
    assert_eq!(e.run(&["--passphrase-stdin", "status"], "wrong passphrase\n").0, 4); // bad proof
    let (code, v) = e.run(&["--passphrase-stdin", "status"], "a long passphrase\n");
    assert_eq!((code, v["key_store"].as_str()), (0, Some("passphrase")));
    assert_eq!(e.run(&["--passphrase-stdin", "lock"], "a long passphrase\n").0, 0);
    assert_eq!(e.run(&["--passphrase-stdin", "status"], "a long passphrase\n").0, 5);
}

/// M2 acceptance: an MCP client under a limited grant reads the 90-day resting-HR trend,
/// writes a derived metric, and the owner sees both in the audit log.
fn mcp_session(e: &Env, token: &str) -> Vec<Value> {
    let mut c = e.cmd().arg("mcp").env("HEALTH_TOKEN", token).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let msgs = [
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "1"}}}),
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
        json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "daily_query", "arguments": {"metric": "loinc:40443-4"}}}),
        json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {"name": "derived_write", "arguments": {
            "algo": "rhr-trend", "version": "1", "metric": {"name": "slope", "unit": "/min/d", "agg": "instant"},
            "inputs": [{"metric": "loinc:40443-4", "from_ms": 0, "to_ms": 1795176000000i64}],
            "rows": [{"metric": "x", "start_ms": 1795176000000i64, "tz_min": 0, "value": 1.0}]}}}),
        json!({"jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": {"name": "daily_query", "arguments": {"metric": "loinc:2345-7"}}}),
    ];
    let mut input = c.stdin.take().unwrap();
    for m in &msgs {
        writeln!(input, "{m}").unwrap();
    }
    drop(input);
    let out = c.wait_with_output().unwrap();
    String::from_utf8(out.stdout).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect()
}

fn check_session(e: &Env, replies: &[Value]) {
    assert_eq!(replies.len(), 5, "{replies:?}"); // the notification got no reply
    assert!(replies[1]["result"]["tools"].as_array().unwrap().iter().all(|t| t["inputSchema"]["type"] == "object"));
    let text = |i: usize| -> Value { serde_json::from_str(replies[i]["result"]["content"][0]["text"].as_str().unwrap()).unwrap() };
    assert_eq!(text(2)["items"].as_array().unwrap().len(), 5);
    assert!(text(2)["withheld"].is_object());
    assert_eq!(text(3)["items"]["metric"], "x:derived/rhr-trend/slope", "{}", text(3));
    assert_eq!((replies[4]["result"]["isError"].as_bool(), text(4)["error"]["code"].as_str()), (Some(true), Some("out_of_scope")));
    let audit = e.run(&["audit"], "").1;
    let ops: Vec<&str> = audit.as_array().unwrap().iter().map(|a| a["op"].as_str().unwrap()).collect();
    assert!(ops.contains(&"daily.query") && ops.contains(&"derived.write"), "{ops:?}");
}

#[test]
fn mcp_over_stdio_without_serve() {
    let e = Env::new();
    e.seed_rhr();
    let token = e.grant(&[]);
    let replies = mcp_session(&e, &token);
    check_session(&e, &replies);
}

#[test]
fn mcp_over_stdio_forwarded_to_serve() {
    let e = Env::new();
    e.seed_rhr();
    let token = e.grant(&[]);
    let (_s, _) = e.serve();
    let replies = mcp_session(&e, &token);
    check_session(&e, &replies);
}

#[test]
fn mcp_needs_a_grant_token() {
    let e = Env::new();
    let out = e.cmd().arg("mcp").stdin(Stdio::null()).output().unwrap();
    assert_eq!(out.status.code(), Some(4));
}

#[test]
fn mcp_over_http_needs_a_grant_and_answers_json() {
    let e = Env::new();
    e.seed_rhr();
    let token = e.grant(&[]);
    let (_s, info) = e.serve();
    let port = info["port"].as_u64().unwrap();
    let agent = format!("Bearer {token}");
    let owner = format!("Bearer {}", info["token"].as_str().unwrap());
    let list = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
    assert_eq!(http(port, "POST", "/mcp", &[("Authorization", &owner)], list).0, 404); // MCP is for delegates
    let (code, v) = http(port, "POST", "/mcp", &[("Authorization", &agent)], list);
    assert_eq!((code, v["result"]["tools"].as_array().map(|t| !t.is_empty())), (200, Some(true)));
    assert_eq!(
        http(port, "POST", "/mcp", &[("Authorization", &agent)], r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).0,
        202
    );
    assert_eq!(http(port, "GET", "/mcp", &[("Authorization", &agent)], "").0, 405);
}

#[test]
fn passphrase_set_is_refused_while_serve_runs() {
    let e = Env::new();
    let (_s, _) = e.serve();
    let (code, v) = e.run(&["--passphrase-stdin", "passphrase", "set"], "a long passphrase\n");
    assert_ne!(code, 0);
    assert!(v.to_string().contains("serve_running"), "{v}");
    // nothing changed: serve still answers the owner without a passphrase
    assert_eq!(e.run(&["status"], "").0, 0);
}

#[test]
fn health_op_paths_are_made_absolute_before_forwarding() {
    let e = Env::new();
    let (_s, _) = e.serve();
    std::fs::write(e.dir.path().join("rows.jsonl"), "{\"metric\":\"loinc:8867-4\",\"start_ms\":1795000000000,\"tz_min\":0,\"value\":60}\n")
        .unwrap();
    let out = e
        .cmd()
        .current_dir(e.dir.path())
        .args(["--json", "op", "import.rows", r#"{"path":"rows.jsonl","source":"test"}"#])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stdout));
}
