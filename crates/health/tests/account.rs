//! The health account through the binary, against a loopback relay that speaks the backend's protocol
//! (debug builds take HEALTH_BACKEND): two computers link with `health link join` / `give`, both show the same
//! identity code, and a lab-signed result arrives through `health inbox`, directly and through a running serve.

use ed25519_dalek::{Signer, SigningKey};
use health_proto::b64::{b64u, unb64u};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Relay {
    log: Vec<String>,
    keys: HashMap<String, String>,
    links: HashMap<String, String>,
    inbox: HashMap<String, Vec<(i64, String)>>,
}

fn reply(r: &mut Relay, method: &str, path: &str, body: &str) -> (u16, Value) {
    r.log.push(format!("{method} {path}"));
    match (method, path) {
        ("POST", "/h/1/key") => {
            let jws: Value = serde_json::from_str(body).unwrap();
            let p = jws["jws"].as_str().unwrap().split('.').nth(1).unwrap();
            let payload: Value = serde_json::from_slice(&unb64u(p).unwrap()).unwrap();
            r.keys.insert(payload["pk"].as_str().unwrap().into(), "x".into());
            (200, json!({}))
        }
        ("POST", p) if p.starts_with("/h/1/link/") => {
            let ch = &p["/h/1/link/".len()..];
            if r.links.contains_key(ch) {
                return (409, json!({ "err": "busy" }));
            }
            let v: Value = serde_json::from_str(body).unwrap();
            r.links.insert(ch.into(), v["ct"].as_str().unwrap().into());
            (200, json!({}))
        }
        ("GET", p) if p.starts_with("/h/1/link/") => match r.links.remove(&p["/h/1/link/".len()..]) {
            Some(ct) => (200, json!({ "ct": ct })),
            None => (404, json!({ "err": "unknown" })),
        },
        ("POST", "/h/1/whoop/token") => (200, json!({ "access_token": "at", "refresh_token": "rt", "expires_in": 3600 })),
        ("GET", "/h/1/whoop/v2/user/profile/basic") => (200, json!({ "user_id": 12345 })),
        ("GET", p) if p.starts_with("/h/1/whoop/v2/user/measurement/body") => {
            (200, json!({ "height_meter": 1.8, "weight_kilogram": 72.5, "max_heart_rate": 190 }))
        }
        ("GET", p) if p.starts_with("/h/1/whoop/") => (200, json!({ "records": [] })),
        ("GET", p) if p.starts_with("/h/1/inbox/") => {
            let rest = &p["/h/1/inbox/".len()..];
            let (pk, after) = rest.split_once("?after=").map_or((rest, i64::MIN), |(k, a)| (k, a.parse().unwrap()));
            if !r.keys.contains_key(pk) {
                return (404, json!({ "err": "unknown" }));
            }
            let items: Vec<Value> =
                r.inbox.get(pk).into_iter().flatten().filter(|(s, _)| *s > after).map(|(s, ct)| json!({ "seq": s, "ct": ct })).collect();
            (200, json!({ "items": items, "more": false }))
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
                let (status, v) = reply(&mut st.lock().unwrap(), &m, &p, &String::from_utf8(b).unwrap());
                let body = v.to_string();
                let mut s = s;
                let _ = write!(s, "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            });
        }
    });
    (base, state)
}

const LAB_SEED: [u8; 32] = [9; 32];

fn did_key(pk: &[u8; 32]) -> String {
    const A: &[u8] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
    let mut n: Vec<u8> = [&[0xed, 0x01][..], pk].concat();
    let mut out = Vec::new();
    while n.iter().any(|&b| b != 0) {
        let mut rem = 0u32;
        for d in n.iter_mut() {
            let cur = rem * 256 + *d as u32;
            *d = (cur / 58) as u8;
            rem = cur % 58;
        }
        out.push(A[rem as usize]);
    }
    out.reverse();
    format!("did:key:z{}", String::from_utf8(out).unwrap())
}

fn lab_did() -> String {
    did_key(&SigningKey::from_bytes(&LAB_SEED).verifying_key().to_bytes())
}

/// A weight result for `uid`, signed by the lab key and sealed to `pk`.
fn lab_result(uid: &str, pk: &str, id: &str, kg: f64) -> String {
    let key = SigningKey::from_bytes(&LAB_SEED);
    let mut doc = json!({ "v": 1, "id": id, "uid": uid, "src": "lab:seca-mbca-01", "at": 1_795_170_000_000i64, "tz": 480,
        "obs": [{ "code": "loinc:29463-7", "v": kg, "u": "kg" }] });
    let bytes = health_proto::sig::jcs(&doc).unwrap();
    doc["sig"] = json!({ "key": lab_did(), "s": b64u(&key.sign(bytes.as_bytes()).to_bytes()) });
    let pk: [u8; 32] = unb64u(pk).unwrap().try_into().unwrap();
    b64u(&health_proto::hpke::seal(&pk, doc.to_string().as_bytes(), b"sibian-health-1", b"", &[7; 32]).unwrap())
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
            .env("HEALTH_NOW", "1795176000000")
            .env("HEALTH_TZ_MIN", "480")
            .env("HEALTH_BACKEND", &self.base)
            .env("HEALTH_LAB_KEYS", format!("{}:lab", lab_did()))
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

#[test]
fn two_computers_link_and_show_the_same_identity_code() {
    let (base, relay) = relay();
    let (old, new) = (Env::new(&base), Env::new(&base));
    // before anything is asked, nothing is sent
    assert_eq!(new.run(&["account"], "").1["account"], Value::Null);
    assert_eq!(new.run(&["inbox"], "").1["status"], "nokey");
    assert!(relay.lock().unwrap().log.is_empty());

    let (uid, pk) = old.identity();
    // refused on the old device: nothing sent, nothing kept on the new one
    let (code, _, given) = link(&old, &new, "n\n", "y\n");
    assert_ne!(given, 0);
    assert_ne!(code, 0);
    assert_eq!(new.run(&["account"], "").1["account"], Value::Null);
    // a "no" at the check code keeps nothing either
    let (code, _, given) = link(&old, &new, "y\n", "n\n");
    assert_eq!((given, code == 0), (0, false));
    assert_eq!(new.run(&["account"], "").1["account"], Value::Null);
    // yes and yes: the same account
    let (code, v, given) = link(&old, &new, "y\n", "y\n");
    assert_eq!((given, code), (0, 0), "{v}");
    assert_eq!(v["id"], uid);
    assert_eq!(new.identity(), (uid.clone(), pk.clone()));

    // a lab result reaches the new computer
    relay.lock().unwrap().inbox.insert(pk.clone(), vec![(1_000, lab_result(&uid, &pk, "0193a5c2-7e1f-7b3a-9c4d-2f6e8a1b5c90", 68.4))]);
    let (code, v) = new.run(&["inbox"], "");
    assert_eq!((code, v["saved"].as_u64()), (0, Some(1)), "{v}");
    let (_, latest) = new.run(&["obs", "latest"], "");
    assert_eq!(latest[0]["value"], 68.4, "{latest}");
    // offline: refused before anything leaves
    let n = relay.lock().unwrap().log.len();
    let (code, v) = new.run(&["--offline", "inbox"], "");
    assert_ne!(code, 0);
    assert_eq!(v["error"]["code"], "offline");
    assert_eq!(relay.lock().unwrap().log.len(), n);
}

#[test]
fn through_a_running_serve_the_inbox_and_the_link_work_the_same() {
    let (base, relay) = relay();
    let (old, new) = (Env::new(&base), Env::new(&base));
    let (uid, pk) = old.identity();
    let mut serve = new.cmd().arg("serve").stdout(Stdio::piped()).spawn().unwrap();
    let mut first = String::new();
    BufReader::new(serve.stdout.take().unwrap()).read_line(&mut first).unwrap();
    let (code, v, given) = link(&old, &new, "y\n", "y\n");
    assert_eq!((given, code), (0, 0), "{v}");
    assert_eq!(new.identity(), (uid.clone(), pk.clone()));
    relay.lock().unwrap().inbox.insert(pk.clone(), vec![(1_000, lab_result(&uid, &pk, "0193a5c2-7e1f-7b3a-9c4d-2f6e8a1b5c91", 70.0))]);
    assert_eq!(new.run(&["inbox"], "").1["saved"], 1);
    assert_eq!(new.run(&["inbox"], "").1["saved"], 0);
    // --offline holds even though the running serve wasn't started with it
    let n = relay.lock().unwrap().log.len();
    assert_eq!(new.run(&["--offline", "inbox"], "").1["error"]["code"], "offline");
    assert_eq!(relay.lock().unwrap().log.len(), n);
    serve.kill().unwrap();
    let _ = serve.wait();
    // the account key is in the profile's vault, sealed: never as text
    let vault = std::fs::read(new.dir.path().join("data/profiles/default/vault")).unwrap();
    assert!(!String::from_utf8_lossy(&vault).contains("amk"));
}

#[test]
fn whoop_sign_in_comes_back_to_this_computer_and_the_first_sync_runs() {
    let (base, relay) = relay();
    let e = Env::new(&base);
    let mut c = e
        .cmd()
        .args(["--json", "whoop", "connect", "--client-id", "my-app"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    c.stdin.take().unwrap().write_all(b"s3cret\n").unwrap();
    let mut err = BufReader::new(c.stderr.take().unwrap());
    let mut url = String::new();
    let mut line = String::new();
    while err.read_line(&mut line).unwrap() > 0 {
        if line.starts_with("https://api.prod.whoop.com/") {
            url = line.trim().to_string();
            break;
        }
        line.clear();
    }
    let state = url.split("state=").nth(1).unwrap().to_string();
    // the browser, coming back from WHOOP's page (wait for the listener)
    let mut answer = String::new();
    for _ in 0..50 {
        if let Ok(mut s) = std::net::TcpStream::connect(("127.0.0.1", 47630)) {
            write!(s, "GET /whoop?code=the-code&state={state} HTTP/1.1\r\nHost: localhost:47630\r\n\r\n").unwrap();
            s.read_to_string(&mut answer).unwrap();
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(answer.contains("return to Health"), "{answer}");
    let out = c.wait_with_output().unwrap();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(out.status.code(), Some(0), "{v}");
    assert!(v["written"].as_u64().unwrap() > 0, "{v}");
    assert_eq!(e.run(&["whoop", "status"], "").1["state"], "connected");
    // the secret never reached a URL
    assert!(!relay.lock().unwrap().log.iter().any(|l| l.contains("s3cret")));
}
