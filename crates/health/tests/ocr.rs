//! A lab report end to end through the binary: component install, `health ocr` (worker process), a batch no
//! query or agent can see, confirm, readings. Direct and through a running serve. Needs the models:
//! HEALTH_OCR_MODELS=<dir with the pinned files>; the no-component case runs without.

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

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
            .env("HEALTH_TZ_MIN", "480")
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
}

fn fixture(name: &str) -> String {
    format!("{}/../health-ocr/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn without_the_component_ocr_says_how_to_get_it() {
    let e = Env::new();
    let (code, v) = e.run(&["ocr", &fixture("cbc.png")], "");
    assert_ne!(code, 0);
    assert_eq!(v["error"]["code"], "no_ocr_component", "{v}");
    assert_eq!(e.run(&["component", "list"], "").1["ready"], false);
}

fn report_flow(e: &Env) {
    // a review batch: nothing visible yet, not to the owner and not to an agent
    let (code, v) = e.run(&["ocr", &fixture("cbc.png")], "");
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["items"], 14);
    assert_eq!(e.run(&["obs", "latest"], "").1, json!([]));
    let (code, v) = e.run(&["grant", "create", "--name", "agent", "--groups", "lab", "--detail", "--days", "3650"], "");
    assert_eq!(code, 0, "{v}");
    let token = v["token"].as_str().unwrap().to_string();
    let mut mcp = e.cmd().arg("mcp").env("HEALTH_TOKEN", &token).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let mut out = BufReader::new(mcp.stdout.take().unwrap());
    let mut ask = |msg: Value| -> Value {
        writeln!(mcp.stdin.as_mut().unwrap(), "{msg}").unwrap();
        let mut line = String::new();
        out.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    };
    ask(
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}}}),
    );
    let seen = ask(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "obs_latest", "arguments": {}}}));
    let text: Value = serde_json::from_str(seen["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(text["items"], json!([]), "an agent must not see an unconfirmed report");

    // the same file again is refused
    let (code, v) = e.run(&["ocr", &fixture("cbc.png")], "");
    assert_ne!(code, 0);
    assert_eq!(v["error"]["code"], "blob_dup", "{v}");

    // confirm what was preselected; units default to the printed ones
    let batch = &e.run(&["review", "list"], "").1[0];
    assert_eq!(batch["candidates"]["dated"], true);
    let choices: Vec<Value> = batch["candidates"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| if i["code"].is_string() { json!({ "code": i["code"] }) } else { json!({ "skip": true }) })
        .collect();
    let req = json!({ "id": batch["import_id"], "choices": choices, "tz_min": 480 });
    let (code, v) = e.run(&["op", "review.confirm_ocr", "-"], &req.to_string());
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["saved"], 14);
    let latest = e.run(&["obs", "latest"], "").1;
    let wbc = latest.as_array().unwrap().iter().find(|p| p["metric"] == "loinc:26464-8").unwrap();
    assert_eq!((wbc["value"].as_f64(), wbc["unit"].as_str()), (Some(6.82), Some("10*3/uL")), "6,820 cells/µL converted");
    let seen = ask(json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "obs_latest", "arguments": {}}}));
    let text: Value = serde_json::from_str(seen["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(text["items"].as_array().unwrap().len(), 14, "confirmed results reach a lab grant");
    drop(mcp.stdin.take());
    let _ = mcp.wait();
}

fn models() -> Option<String> {
    std::env::var("HEALTH_OCR_MODELS").ok()
}

#[test]
fn a_report_through_the_cli() {
    let Some(m) = models() else { return eprintln!("skipped: set HEALTH_OCR_MODELS") };
    let e = Env::new();
    assert_eq!(e.run(&["component", "install", "ocr", "--from", &m], "").1["ready"], true);
    report_flow(&e);
}

#[test]
fn a_report_through_serve() {
    let Some(m) = models() else { return eprintln!("skipped: set HEALTH_OCR_MODELS") };
    let e = Env::new();
    assert_eq!(e.run(&["component", "install", "ocr", "--from", &m], "").1["ready"], true);
    let mut serve = e.cmd().arg("serve").stdout(Stdio::piped()).spawn().unwrap();
    let mut line = String::new();
    BufReader::new(serve.stdout.take().unwrap()).read_line(&mut line).unwrap();
    // serve holds the writer lock: every call below is forwarded, and serve itself records the batch
    report_flow(&e);
    let _ = serve.kill();
    let _ = serve.wait();
}

#[test]
fn a_crashing_worker_leaves_the_core_and_the_data_untouched() {
    let Some(m) = models() else { return eprintln!("skipped: set HEALTH_OCR_MODELS") };
    let e = Env::new();
    assert_eq!(e.run(&["component", "install", "ocr", "--from", &m], "").1["ready"], true);
    assert_eq!(e.run(&["add", "loinc:29463-7", "70", "--unit", "kg"], "").0, 0);
    let mut serve = e.cmd().arg("serve").stdout(Stdio::piped()).spawn().unwrap();
    let mut line = String::new();
    BufReader::new(serve.stdout.take().unwrap()).read_line(&mut line).unwrap();
    let before = e.run(&["status"], "").1;
    let out = e.cmd().args(["--json", "ocr", &fixture("cbc.png")]).env("HEALTH_OCR_WORKER_CRASH", "1").output().unwrap();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["error"]["code"], "ocr_crashed", "{v}");
    // serve still answers, nothing was written, and the next import works
    let after = e.run(&["status"], "").1;
    assert_eq!((after["observations"].clone(), after["imports"].clone()), (before["observations"].clone(), before["imports"].clone()));
    let (code, v) = e.run(&["ocr", &fixture("cbc.png")], "");
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["items"], 14);
    let _ = serve.kill();
    let _ = serve.wait();
}

/// Serves the files of `dir` by name over loopback HTTP; returns the base URL.
fn file_server(dir: String) -> String {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for s in l.incoming().flatten() {
            let dir = dir.clone();
            std::thread::spawn(move || {
                let mut r = BufReader::new(s.try_clone().unwrap());
                let mut first = String::new();
                r.read_line(&mut first).unwrap();
                let mut line = String::new();
                while r.read_line(&mut line).unwrap_or(0) > 2 {
                    line.clear();
                }
                let name = first.split_whitespace().nth(1).unwrap_or("/").trim_start_matches('/').to_string();
                let mut s = s;
                match std::fs::read(format!("{dir}/{name}")) {
                    Ok(b) => {
                        let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", b.len());
                        let _ = s.write_all(&b);
                    }
                    Err(_) => {
                        let _ = write!(s, "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                    }
                }
            });
        }
    });
    format!("http://127.0.0.1:{port}")
}

#[test]
fn the_component_installs_by_download_and_reads_a_report() {
    let Some(m) = models() else { return eprintln!("skipped: set HEALTH_OCR_MODELS") };
    let e = Env::new();
    let mirror = file_server(m);
    let out = e.cmd().args(["--json", "component", "install", "ocr"]).env("HEALTH_COMPONENT_MIRROR", &mirror).output().unwrap();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["ready"], true, "{v}");
    assert_eq!(v["installed"].as_array().unwrap().len(), 5, "all five pinned files, matching included");
    // installed again: nothing to fetch
    let out = e.cmd().args(["--json", "component", "install", "ocr"]).env("HEALTH_COMPONENT_MIRROR", &mirror).output().unwrap();
    assert_eq!(serde_json::from_slice::<Value>(&out.stdout).unwrap()["installed"], json!([]));
    let (code, v) = e.run(&["ocr", &fixture("hbsab.png")], "");
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["items"], 2);
}
