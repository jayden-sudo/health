//! `health serve`: the single writer, on 127.0.0.1 only (plan §2.1, §4.2). Thread per request; one profile behind a mutex.
//! Checks, in order: no Origin (no browser page may call it), Host = 127.0.0.1/localhost:<port>, then the caller:
//! owner token (keychain mode: also in serve.json, 0600) or passphrase proof (passphrase mode), or a grant token.

use crate::{keys, mcp};
use health_api::{Caller, Ctx, call};
use health_core::keys::{KEYWRAP, PassphraseStore};
use health_core::profile::{Profile, create_private};
use health_core::{Error, Kind, Result};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub struct Opts {
    pub data_dir: PathBuf,
    pub profile: String,
    pub port: u16,
    pub idle_lock: Duration,
}

struct State {
    data_dir: PathBuf,
    name: String,
    port: u16,
    id: String,
    owner_token: String,
    passphrase_mode: bool,
    open: Mutex<Option<Profile>>,
    proof: Mutex<Option<String>>,
    events: Mutex<Vec<Sender<String>>>,
    last_owner: Mutex<Instant>,
    unlock_gate: Mutex<()>,
}

fn ct_eq(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub fn status_of(k: Kind) -> u16 {
    match k {
        Kind::Usage => 400,
        Kind::Invalid => 422,
        Kind::Denied => 403,
        Kind::Locked => 423,
        Kind::NotFound => 404,
        Kind::Conflict => 409,
        Kind::Other => 500,
    }
}

type Reply = (u16, Body);
enum Body {
    Json(Value),
    Empty,
    Events(Receiver<String>),
}

fn err(e: &Error) -> Reply {
    (status_of(e.kind), Body::Json(json!({ "error": { "code": e.code, "message": e.detail } })))
}

impl State {
    fn emit(&self, event: &str, data: Value) {
        let msg = format!("event: {event}\ndata: {data}\n\n");
        self.events.lock().unwrap().retain(|tx| tx.send(msg.clone()).is_ok());
    }

    fn caller(&self, headers: &[(String, String)], profile: Option<&Profile>, now: i64) -> Result<Caller> {
        let get = |k: &str| headers.iter().find(|(h, _)| h.eq_ignore_ascii_case(k)).map(|(_, v)| v.as_str());
        if let Some(p) = get("X-Health-Proof") {
            if profile.is_none() {
                return Err(Error::new(Kind::Locked, "locked", "serve is locked: run `health unlock`"));
            }
            if self.proof.lock().unwrap().as_deref().is_some_and(|x| ct_eq(x, p)) {
                return Ok(Caller::Owner);
            }
            return Err(Error::new(Kind::Denied, "denied", "bad passphrase proof"));
        }
        let token =
            get("Authorization").and_then(|v| v.strip_prefix("Bearer ")).ok_or_else(|| Error::new(Kind::Denied, "denied", "no token"))?;
        if ct_eq(token, &self.owner_token) {
            return Ok(Caller::Owner);
        }
        let p = profile.ok_or_else(|| Error::new(Kind::Locked, "locked", "serve is locked"))?;
        Ok(Caller::Delegate(p.access.resolve(token, now)?))
    }

    fn handle(self: &Arc<Self>, method: &str, url: &str, headers: &[(String, String)], body: &[u8]) -> Reply {
        let get = |k: &str| headers.iter().find(|(h, _)| h.eq_ignore_ascii_case(k)).map(|(_, v)| v.as_str());
        if get("Origin").is_some() {
            return err(&Error::new(Kind::Denied, "origin", "browser origins are refused"));
        }
        let host_ok = get("Host").is_some_and(|h| h == format!("127.0.0.1:{}", self.port) || h == format!("localhost:{}", self.port));
        if !host_ok {
            return err(&Error::new(Kind::Denied, "host", "wrong Host"));
        }
        let path = url.split('?').next().unwrap_or("");
        let now = crate::time::now_ms();
        match (method, path) {
            ("GET", "/v1/meta") => {
                return (200, Body::Json(json!({ "core": env!("CARGO_PKG_VERSION"), "api": 1, "schema": 1, "id": self.id })));
            }
            ("POST", "/v1/unlock") => return self.unlock(body),
            ("GET", "/mcp") => return (405, Body::Empty),
            _ => {}
        }
        let guard = self.open.lock().unwrap();
        let caller = match self.caller(headers, guard.as_ref(), now) {
            Ok(c) => c,
            Err(e) => return err(&e),
        };
        let owner = matches!(caller, Caller::Owner);
        if owner {
            *self.last_owner.lock().unwrap() = Instant::now();
        }
        match (method, path) {
            ("POST", "/v1/lock") if owner => {
                drop(guard);
                if !self.passphrase_mode {
                    return err(&Error::conflict("no_passphrase", "only a passphrase-locked profile can be locked"));
                }
                self.lock();
                (200, Body::Json(json!({ "locked": true })))
            }
            ("GET", "/v1/events") if owner => {
                let (tx, rx) = channel();
                let _ = tx.send(": connected\n\n".into());
                self.events.lock().unwrap().push(tx);
                (200, Body::Events(rx))
            }
            ("POST", "/mcp") if !owner => {
                let Some(profile) = guard.as_ref() else { return err(&Error::new(Kind::Locked, "locked", "")) };
                let Ok(msg) = serde_json::from_slice::<Value>(body) else { return err(&Error::new(Kind::Usage, "bad_json", "")) };
                let run = |op: &str, args: Value| {
                    let mut ctx = Ctx::new(profile, now, caller.clone());
                    call(&mut ctx, op, args)
                };
                let mut wrote = false;
                let mut run = |op: &str, args: Value| {
                    let r = run(op, args);
                    wrote |= r.is_ok() && health_api::find(op).is_ok_and(|o| o.write);
                    r
                };
                let reply = mcp::handle(&msg, &mut run);
                drop(guard);
                if wrote {
                    self.emit("changed", json!({ "op": "mcp" }));
                }
                match reply {
                    Some(r) => (200, Body::Json(r)),
                    None => (202, Body::Empty),
                }
            }
            ("POST", p) if p.starts_with("/v1/op/") => {
                let Some(profile) = guard.as_ref() else { return err(&Error::new(Kind::Locked, "locked", "serve is locked")) };
                let name = &p["/v1/op/".len()..];
                let req = if body.is_empty() { Ok(json!({})) } else { serde_json::from_slice::<Value>(body) };
                let Ok(req) = req else { return err(&Error::new(Kind::Usage, "bad_json", "")) };
                let mut ctx = Ctx::new(profile, now, caller);
                ctx.dry_run = owner && get("X-Health-Dry-Run") == Some("1");
                let res = call(&mut ctx, name, req);
                if owner {
                    *self.last_owner.lock().unwrap() = Instant::now(); // idle time counts from the end of the owner's op
                }
                let wrote = res.is_ok() && !ctx.dry_run && health_api::find(name).is_ok_and(|o| o.write);
                drop(guard);
                match res {
                    Ok(o) => {
                        if wrote {
                            self.emit("changed", json!({ "op": name }));
                        }
                        (200, Body::Json(json!({ "value": o.value, "withheld": o.withheld })))
                    }
                    Err(e) => err(&e),
                }
            }
            _ => err(&Error::new(Kind::NotFound, "not_found", path.to_string())),
        }
    }

    fn unlock(&self, body: &[u8]) -> Reply {
        if !self.passphrase_mode {
            return err(&Error::conflict("not_locked", "this profile has no passphrase"));
        }
        let _one_at_a_time = self.unlock_gate.lock().unwrap();
        let p = serde_json::from_slice::<Value>(body).ok().and_then(|v| v["passphrase"].as_str().map(String::from)).unwrap_or_default();
        let dir = match health_core::profile::profile_dir(&self.data_dir, &self.name) {
            Ok(d) => d,
            Err(e) => return err(&e),
        };
        let store = PassphraseStore::new(&dir.join(KEYWRAP), &p);
        let fail = |e: Error| {
            std::thread::sleep(Duration::from_secs(1));
            err(&e)
        };
        // One key derivation, before taking `open`, so other requests never wait on it.
        let proof = match store.proof() {
            Ok(p) => p,
            Err(e) => return fail(e),
        };
        let mut open = self.open.lock().unwrap();
        if open.is_some() {
            // Already unlocked: only the same passphrase gives the same proof; nothing changes on a miss.
            let same = self.proof.lock().unwrap().as_deref().is_some_and(|cur| ct_eq(cur, &proof));
            drop(open);
            if !same {
                return fail(Error::new(Kind::Locked, "bad_passphrase", ""));
            }
        } else {
            match Profile::open(&self.data_dir, &self.name, &store, true, crate::time::now_ms()) {
                // the proof is stored together with the open profile: never one without the other
                Ok(p) => {
                    *self.proof.lock().unwrap() = Some(proof);
                    *open = Some(p);
                    drop(open);
                }
                Err(e) => {
                    drop(open);
                    return fail(e);
                }
            }
        }
        *self.last_owner.lock().unwrap() = Instant::now();
        self.emit("unlocked", json!({}));
        (200, Body::Json(json!({ "locked": false })))
    }

    fn lock(&self) {
        if self.passphrase_mode {
            let mut open = self.open.lock().unwrap();
            *self.proof.lock().unwrap() = None;
            *open = None; // drops health.db, access.db and the key, with the proof, in one step
            drop(open);
            self.emit("locked", json!({}));
        }
    }
}

pub fn run(o: Opts) -> Result<()> {
    let dir = health_core::profile::profile_dir(&o.data_dir, &o.profile)?;
    if crate::client::probe(&dir).is_some() {
        return Err(Error::conflict("running", "a serve is already running for this profile"));
    }
    let passphrase_mode = dir.join(KEYWRAP).exists();
    let open = if passphrase_mode {
        None
    } else {
        Some(Profile::open(&o.data_dir, &o.profile, keys::from_env().as_ref(), true, crate::time::now_ms())?)
    };
    let server = tiny_http::Server::http(("127.0.0.1", o.port)).map_err(|e| Error::other("listen", e.to_string()))?;
    let port = server.server_addr().to_ip().map(|a| a.port()).ok_or_else(|| Error::other("listen", "no port"))?;
    let state = Arc::new(State {
        data_dir: o.data_dir.clone(),
        name: o.profile.clone(),
        port,
        id: hex::encode(health_core::aead::random::<16>()),
        owner_token: hex::encode(health_core::aead::random::<32>()),
        passphrase_mode,
        open: Mutex::new(open),
        proof: Mutex::new(None),
        events: Mutex::new(Vec::new()),
        last_owner: Mutex::new(Instant::now()),
        unlock_gate: Mutex::new(()),
    });
    // serve.json: how the CLI finds us. In passphrase mode it never holds the owner token (plan §3.3).
    let file = dir.join("serve.json");
    let _ = std::fs::remove_file(&file);
    let mut info = json!({ "port": port, "pid": std::process::id(), "id": state.id });
    if !passphrase_mode {
        info["token"] = json!(state.owner_token);
    }
    std::io::Write::write_all(&mut create_private(&file)?, info.to_string().as_bytes())?;
    println!("{}", json!({ "port": port, "token": state.owner_token, "id": state.id, "locked": passphrase_mode }));
    let _ = std::io::Write::flush(&mut std::io::stdout());

    if passphrase_mode {
        let s = state.clone();
        let idle = o.idle_lock;
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(Duration::from_secs(5).min(idle));
                if s.open.lock().unwrap().is_some() && s.last_owner.lock().unwrap().elapsed() >= idle {
                    s.lock();
                }
            }
        });
    }
    for mut rq in server.incoming_requests() {
        let s = state.clone();
        std::thread::spawn(move || {
            let headers: Vec<(String, String)> = rq.headers().iter().map(|h| (h.field.to_string(), h.value.to_string())).collect();
            let mut body = Vec::new();
            let _ = std::io::Read::take(rq.as_reader(), 64 * 1024 * 1024).read_to_end(&mut body);
            let (status, b) = s.handle(rq.method().as_str(), rq.url(), &headers, &body);
            let ct = |v: &str| tiny_http::Header::from_bytes("Content-Type", v).unwrap();
            let _ = match b {
                Body::Json(v) => {
                    rq.respond(tiny_http::Response::from_string(v.to_string()).with_status_code(status).with_header(ct("application/json")))
                }
                Body::Empty => rq.respond(tiny_http::Response::empty(status)),
                // Straight to the socket, flushed per event: tiny_http's Response would buffer a streamed body.
                Body::Events(rx) => {
                    let mut w = rq.into_writer();
                    let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n";
                    let mut ok = w.write_all(head.as_bytes()).and_then(|_| w.flush()).is_ok();
                    while ok {
                        let msg = rx.recv_timeout(Duration::from_secs(15)).unwrap_or_else(|_| ": ping\n\n".into());
                        ok = w.write_all(msg.as_bytes()).and_then(|_| w.flush()).is_ok();
                    }
                    Ok(())
                }
            };
        });
    }
    let _ = std::fs::remove_file(&file);
    Ok(())
}

use std::io::{Read as _, Write as _};
