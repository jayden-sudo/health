//! LAN sync end to end (docs/lan-sync.md S4): two linked computers, each running `health serve`, find each other
//! through a loopback relay that speaks `/h/1/lan` (debug builds take HEALTH_BACKEND), connect over WebRTC on
//! loopback (HEALTH_LAN_LOOPBACK), ask before their first sync, and then keep each other's data equal.

use health_proto::b64::b64u;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Default)]
struct Relay {
    links: HashMap<String, String>,
    lan: HashMap<String, Vec<(i64, String)>>,
    seq: i64,
    posts: usize,
}

fn reply(st: &Arc<Mutex<Relay>>, method: &str, path: &str, body: &str) -> (u16, Value) {
    let (path, query) = path.split_once('?').unwrap_or((path, ""));
    let q = |k: &str| query.split('&').find_map(|kv| kv.strip_prefix(&format!("{k}="))).unwrap_or("0").to_string();
    match (method, path) {
        ("POST", "/h/1/key") => (200, json!({})),
        ("POST", p) if p.starts_with("/h/1/link/") => {
            let v: Value = serde_json::from_str(body).unwrap();
            st.lock().unwrap().links.insert(p["/h/1/link/".len()..].into(), v["ct"].as_str().unwrap().into());
            (200, json!({}))
        }
        ("GET", p) if p.starts_with("/h/1/link/") => match st.lock().unwrap().links.remove(&p["/h/1/link/".len()..]) {
            Some(ct) => (200, json!({ "ct": ct })),
            None => (404, json!({ "err": "unknown" })),
        },
        ("POST", p) if p.starts_with("/h/1/lan/") => {
            let v: Value = serde_json::from_str(body).unwrap();
            let mut r = st.lock().unwrap();
            r.seq += 1;
            r.posts += 1;
            let seq = r.seq;
            r.lan.entry(p["/h/1/lan/".len()..].into()).or_default().push((seq, v["ct"].as_str().unwrap().into()));
            (200, json!({ "seq": seq }))
        }
        ("GET", p) if p.starts_with("/h/1/lan/") => {
            let rid = &p["/h/1/lan/".len()..];
            let after: i64 = q("after").parse().unwrap();
            let until = Instant::now() + Duration::from_secs(q("wait").parse::<u64>().unwrap().min(30));
            loop {
                let items: Vec<Value> = st
                    .lock()
                    .unwrap()
                    .lan
                    .get(rid)
                    .into_iter()
                    .flatten()
                    .filter(|(s, _)| *s > after)
                    .map(|(s, ct)| json!({ "seq": s, "ct": ct }))
                    .collect();
                if !items.is_empty() || Instant::now() >= until {
                    return (200, json!({ "items": items, "more": false }));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        _ => (404, json!({ "err": "unknown" })),
    }
}

fn relay() -> (String, Arc<Mutex<Relay>>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://127.0.0.1:{}", l.local_addr().unwrap().port());
    let state = Arc::new(Mutex::new(Relay::default()));
    let st = state.clone();
    std::thread::spawn(move || {
        for s in l.incoming().flatten() {
            let st = st.clone();
            std::thread::spawn(move || {
                let mut r = BufReader::new(s.try_clone().unwrap());
                let mut first = String::new();
                r.read_line(&mut first).unwrap();
                let (mut len, mut line) = (0, String::new());
                while r.read_line(&mut line).unwrap_or(0) > 2 {
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        len = v.trim().parse().unwrap();
                    }
                    line.clear();
                }
                let mut b = vec![0; len];
                r.read_exact(&mut b).unwrap();
                let mut it = first.split_whitespace();
                let (m, p) = (it.next().unwrap().to_string(), it.next().unwrap().to_string());
                let (status, v) = reply(&st, &m, &p, &String::from_utf8(b).unwrap());
                let body = v.to_string();
                let mut s = s;
                let _ = write!(s, "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            });
        }
    });
    (base, state)
}

struct Env {
    dir: tempfile::TempDir,
    base: String,
}

impl Env {
    fn new(base: &str) -> Env {
        let e = Env { dir: tempfile::tempdir().unwrap(), base: base.into() };
        assert_eq!(e.run(&["init"], "").0, 0);
        e
    }
    fn cmd(&self) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_health"));
        c.env("HEALTH_DATA_DIR", self.dir.path().join("data"))
            .env("HEALTH_ROOT_KEY_FILE", self.dir.path().join("root.key"))
            .env("HEALTH_TZ_MIN", "480")
            .env("HEALTH_BACKEND", &self.base)
            .env("HEALTH_NO_BROWSER", "1")
            .env_remove("HEALTH_OFFLINE")
            .env_remove("HEALTH_TOKEN");
        c
    }
    fn run(&self, args: &[&str], stdin: &str) -> (i32, Value) {
        let mut child =
            self.cmd().arg("--json").args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        child.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
        let out = child.wait_with_output().unwrap();
        let text = String::from_utf8(out.stdout).unwrap();
        (out.status.code().unwrap(), serde_json::from_str(&text).unwrap_or(Value::String(text)))
    }
    fn identity(&self) -> (String, String) {
        let (code, v) = self.run(&["account", "register"], "");
        assert_eq!(code, 0, "{v}");
        let (uid, pk) = health_proto::codes::read_identity(v["identity"].as_str().unwrap()).unwrap();
        (uid, b64u(&pk))
    }
}

/// `health link join` on `new`, answering the check-code question with `answer`; `give` runs on `old`.
fn link(old: &Env, new: &Env, give_answer: &str, answer: &str) -> (i32, Value, i32) {
    let mut join =
        new.cmd().args(["--json", "link", "join"]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let mut err = BufReader::new(join.stderr.take().unwrap());
    let mut code = String::new();
    let mut line = String::new();
    while err.read_line(&mut line).unwrap() > 0 {
        if line.starts_with("ZUHL1:") {
            code = line.trim().to_string();
            break;
        }
        line.clear();
    }
    let (given, _) = old.run(&["link", "give", &code], give_answer);
    std::thread::spawn(move || std::io::copy(&mut err, &mut std::io::sink()));
    if given != 0 {
        // nothing was handed over: the waiting side would wait out its 10 minutes
        join.kill().unwrap();
        let out = join.wait_with_output().unwrap();
        return (out.status.code().unwrap_or(-1), Value::Null, given);
    }
    join.stdin.take().unwrap().write_all(answer.as_bytes()).unwrap();
    let out = join.wait_with_output().unwrap();
    let v = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
    (out.status.code().unwrap(), v, given)
}

struct Serve {
    child: Child,
    port: u16,
    token: String,
}

impl Drop for Serve {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn serve(e: &Env) -> Serve {
    let mut child = e.cmd().env("HEALTH_LAN_LOOPBACK", "1").arg("serve").stdout(Stdio::piped()).spawn().unwrap();
    let mut first = String::new();
    BufReader::new(child.stdout.take().unwrap()).read_line(&mut first).unwrap();
    let v: Value = serde_json::from_str(&first).unwrap();
    Serve { child, port: v["port"].as_u64().unwrap() as u16, token: v["token"].as_str().unwrap().into() }
}

impl Serve {
    fn op(&self, name: &str, body: Value) -> Value {
        let mut s = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        let b = body.to_string();
        write!(
            s,
            "POST /v1/op/{name} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{b}",
            self.port,
            self.token,
            b.len()
        )
        .unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).unwrap();
        let body = out.split("\r\n\r\n").nth(1).unwrap_or("");
        serde_json::from_str::<Value>(body).map(|v| v["value"].clone()).unwrap_or(Value::Null)
    }

    fn live(&self) -> Value {
        self.op("sync.live", json!({}))
    }
}

fn until(what: &str, secs: u64, mut f: impl FnMut() -> bool) {
    let start = Instant::now();
    while !f() {
        assert!(start.elapsed() < Duration::from_secs(secs), "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn state(live: &Value) -> String {
    live["peers"][0]["state"].as_str().unwrap_or("").to_string()
}

#[test]
fn two_linked_computers_find_each_other_ask_once_then_sync_both_ways() {
    let (base, relay) = relay();
    let (old, new) = (Env::new(&base), Env::new(&base));
    old.identity();
    let (code, v, given) = link(&old, &new, "y\n", "y\n");
    assert_eq!((given, code), (0, 0), "{v}");
    assert_eq!(old.run(&["add", "loinc:2345-7", "5.5", "--unit", "mmol/L", "--at", "2026-11-11T07:00"], "").0, 0);
    let (a, b) = (serve(&old), serve(&new));
    // neither has synced before: each asks (§10)
    until("both to ask", 60, || state(&a.live()) == "asking" && state(&b.live()) == "asking");
    assert_eq!(a.live()["peers"][0]["asking"]["ask"], "join");
    // one answers, the other only after a while (a person): waiting on a person is no stall (S6, 2026-11-24)
    let answer = |s: &Serve| {
        let dev = s.live()["peers"][0]["device"].as_str().unwrap().to_string();
        s.op("sync.answer", json!({ "device": dev, "allow": true }));
    };
    answer(&b);
    std::thread::sleep(Duration::from_secs(12));
    assert_ne!(state(&b.live()), "ended", "{}", b.live());
    answer(&a);
    until("up to date", 60, || state(&a.live()) == "up_to_date" && state(&b.live()) == "up_to_date");
    let latest = |s: &Serve| s.op("obs.latest", json!({}));
    // stored in the measure's unit: 5.5 mmol/L of glucose is 99.0858 mg/dL
    assert_eq!(latest(&b)[0]["value"], 99.0858, "{}", latest(&b));
    // a change on the other side travels back on the open channel
    let r = b.op("manual.enter", json!({ "entries": [{ "metric": "loinc:29463-7", "value": 70, "unit": "kg", "at": { "ms": 1_795_000_000_000i64, "tz_min": 480 } }] }));
    assert!(!r.is_null(), "manual.enter");
    until("the new reading on the first computer", 60, || latest(&a).as_array().is_some_and(|v| v.len() == 2));
    assert!(b.op("sync.state", json!({}))["joined"] == true);
    // nothing in the relay holds data: only sealed signalling
    let r = relay.lock().unwrap();
    assert!(r.posts >= 4, "hellos, an offer and an answer");
}

#[test]
fn two_sync_pipes_joined_end_to_end_sync_like_a_channel() {
    // what the web's cross-implementation test does with one side replaced by its own Session
    let (a, b) = (Env::new("http://127.0.0.1:9"), Env::new("http://127.0.0.1:9"));
    assert_eq!(a.run(&["add", "loinc:29463-7", "70.1", "--unit", "kg"], "").0, 0);
    assert_eq!(b.run(&["add", "loinc:8867-4", "61"], "").0, 0);
    let spawn = |e: &Env, extra: &[&str]| {
        e.cmd()
            .args(["sync-pipe", "--allow", "--batch-rows", "3"])
            .args(extra)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    };
    let (mut pa, mut pb) = (spawn(&a, &[]), spawn(&b, &["--no-files"]));
    let (ia, oa) = (pa.stdin.take().unwrap(), pa.stdout.take().unwrap());
    let (ib, ob) = (pb.stdin.take().unwrap(), pb.stdout.take().unwrap());
    let t1 = std::thread::spawn(move || {
        let (mut from, mut to) = (oa, ib);
        let _ = std::io::copy(&mut from, &mut to);
    });
    let t2 = std::thread::spawn(move || {
        let (mut from, mut to) = (ob, ia);
        let _ = std::io::copy(&mut from, &mut to);
    });
    let (ra, rb) = (pa.wait_with_output().unwrap(), pb.wait_with_output().unwrap());
    let _ = (t1.join(), t2.join());
    let last = |o: &std::process::Output| -> Value {
        serde_json::from_str(String::from_utf8_lossy(&o.stderr).lines().last().unwrap_or("null")).unwrap()
    };
    let (va, vb) = (last(&ra), last(&rb));
    assert_eq!(va["status"]["state"], "up_to_date", "{va}");
    assert_eq!(vb["status"]["state"], "up_to_date", "{vb}");
    assert_eq!(va["digest"], vb["digest"]);
}
