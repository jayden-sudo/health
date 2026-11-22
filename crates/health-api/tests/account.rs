//! The health account, linking and the inbox against an in-memory mock of the health backend that checks
//! signatures as the real one does (protocol 0.1) and can issue lab-signed results.

use ed25519_dalek::{Signer, SigningKey};
use health_api::account::{Backend, Config, Net, Reply, Session, call_net};
use health_api::{Caller, Ctx, call};
use health_core::Result;
use health_core::keys::MemoryStore;
use health_core::profile::Profile;
use health_proto::b64::{b64u, unb64u};
use health_proto::result::LabKey;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Mutex;

const NOW: i64 = 1_795_176_000_000;
const AUD: &str = "relay.test";

#[derive(Default)]
struct Relay {
    /// every request: "GET /path" or "POST /path"
    log: Vec<String>,
    keys: HashMap<String, String>,
    links: HashMap<String, String>,
    inbox: HashMap<String, Vec<(i64, String)>>,
    /// requests refused with this code once
    fail_next: Option<String>,
    /// WHOOP relay: refresh tokens handed out (each works once), and the token forms received
    whoop_refresh: Vec<String>,
    whoop_forms: Vec<String>,
}

#[derive(Default)]
struct Mock(Mutex<Relay>);

/// Checks a signed request as the backend does; returns (account id, payload).
fn verify(body: &str, typ: &str) -> std::result::Result<(String, Value), String> {
    let v: Value = serde_json::from_str(body).map_err(|_| "bad")?;
    let jws = v["jws"].as_str().ok_or("bad")?;
    let parts: Vec<&str> = jws.split('.').collect();
    let [h, p, s] = parts[..] else { return Err("bad".into()) };
    let head: Value = serde_json::from_slice(&unb64u(h).ok_or("bad")?).map_err(|_| "bad")?;
    if head["typ"] != typ || head["alg"] != "EdDSA" || head.as_object().unwrap().len() != 3 {
        return Err("bad".into());
    }
    let x = unb64u(head["jwk"]["x"].as_str().ok_or("bad")?).ok_or("bad")?;
    if !health_proto::sig::ed_verify(&x, format!("{h}.{p}").as_bytes(), &unb64u(s).ok_or("bad")?) {
        return Err("auth".into());
    }
    let payload: Value = serde_json::from_slice(&unb64u(p).unwrap()).map_err(|_| "bad")?;
    if payload["aud"] != AUD || payload["at"] != NOW || payload["jti"].as_str().map(str::len) != Some(22) {
        return Err("bad".into());
    }
    use sha2::Digest;
    Ok((hex::encode(sha2::Sha256::digest(&x)), payload))
}

impl Backend for Mock {
    fn get(&self, path: &str) -> Reply {
        let mut r = self.0.lock().unwrap();
        r.log.push(format!("GET {path}"));
        if let Some(c) = r.fail_next.take() {
            return Err(c);
        }
        if let Some(ch) = path.strip_prefix("/h/1/link/") {
            return r.links.remove(ch).map(|ct| json!({ "ct": ct })).ok_or_else(|| "unknown".into());
        }
        let rest = path.strip_prefix("/h/1/inbox/").ok_or("bad")?;
        let (pk, after) = match rest.split_once("?after=") {
            Some((pk, a)) => (pk, a.parse::<i64>().unwrap()),
            None => (rest, i64::MIN),
        };
        if !r.keys.contains_key(pk) {
            return Err("unknown".into());
        }
        let items: Vec<Value> =
            r.inbox.get(pk).into_iter().flatten().filter(|(s, _)| *s > after).map(|(s, ct)| json!({ "seq": s, "ct": ct })).collect();
        Ok(json!({ "items": items, "more": false }))
    }
    fn post(&self, path: &str, body: &str) -> Reply {
        let mut r = self.0.lock().unwrap();
        r.log.push(format!("POST {path}"));
        if path == "/h/1/key" {
            let (uid, p) = verify(body, "health-key")?;
            r.keys.insert(p["pk"].as_str().unwrap().to_string(), uid);
            return Ok(json!({}));
        }
        if path == "/h/1/self" {
            let (uid, p) = verify(body, "health-self")?;
            if r.keys.get(p["pk"].as_str().unwrap()) != Some(&uid) {
                return Err("unknown".into());
            }
            return Ok(json!({ "code": "160010", "until": NOW + 3_600_000 }));
        }
        let ch = path.strip_prefix("/h/1/link/").ok_or("bad")?;
        if r.links.contains_key(ch) {
            return Err("busy".into());
        }
        let ct: Value = serde_json::from_str(body).unwrap();
        r.links.insert(ch.to_string(), ct["ct"].as_str().unwrap().to_string());
        Ok(json!({}))
    }
    fn raw(&self, method: &str, path: &str, headers: &[(&str, &str)], body: &str) -> std::result::Result<(u16, Value), String> {
        let mut r = self.0.lock().unwrap();
        r.log.push(format!("{method} {path}"));
        if path == "/h/1/whoop/token" {
            r.whoop_forms.push(body.to_string());
            let field = |k: &str| body.split('&').find_map(|kv| kv.strip_prefix(&format!("{k}="))).map(String::from);
            if field("grant_type").as_deref() == Some("refresh_token") {
                // a refresh token works once
                let rt = field("refresh_token").unwrap_or_default();
                let Some(i) = r.whoop_refresh.iter().position(|t| *t == rt) else { return Ok((400, json!({ "error": "invalid_grant" }))) };
                r.whoop_refresh.remove(i);
            } else if field("code").as_deref() != Some("the-code") {
                return Ok((400, json!({ "error": "invalid_grant" })));
            }
            let n = r.whoop_forms.len();
            r.whoop_refresh.push(format!("rt{n}"));
            return Ok((200, json!({ "access_token": format!("at{n}"), "refresh_token": format!("rt{n}"), "expires_in": 3600 })));
        }
        let bearer = headers.iter().find(|(k, _)| *k == "authorization").map(|(_, v)| v.to_string()).unwrap_or_default();
        if !bearer.starts_with("Bearer at") {
            return Ok((401, json!({})));
        }
        let bundle: Value = serde_json::from_str(include_str!("../../health-core/tests/fixtures/whoop-bundle.json")).unwrap();
        let p = path.split('?').next().unwrap();
        Ok((
            200,
            match p {
                "/h/1/whoop/v2/user/profile/basic" => json!({ "user_id": 12345 }),
                "/h/1/whoop/v2/cycle" => json!({ "records": bundle["cycles"] }),
                "/h/1/whoop/v2/activity/sleep" => json!({ "records": bundle["sleeps"] }),
                "/h/1/whoop/v2/recovery" => json!({ "records": bundle["recoveries"] }),
                "/h/1/whoop/v2/activity/workout" => json!({ "records": bundle["workouts"] }),
                "/h/1/whoop/v2/user/measurement/body" => bundle["body"].clone(),
                _ => return Ok((404, json!({}))),
            },
        ))
    }
}

/// A did:key (base58btc of 0xed01 ‖ key).
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

struct Lab {
    key: SigningKey,
    did: String,
}

impl Lab {
    fn new() -> Lab {
        let key = SigningKey::from_bytes(&[9; 32]);
        let did = did_key(&key.verifying_key().to_bytes());
        Lab { key, did }
    }
    /// A result file for `uid`, signed and sealed to the inbox key `pk` (base64url).
    fn seal(&self, uid: &str, id: &str, pk: &str, obs: Value) -> String {
        let mut doc = json!({ "v": 1, "id": id, "uid": uid, "src": "lab:seca-mbca-01", "at": NOW - 3_600_000, "tz": 480, "obs": obs });
        let bytes = health_proto::sig::jcs(&doc).unwrap();
        doc["sig"] = json!({ "key": self.did, "s": b64u(&self.key.sign(bytes.as_bytes()).to_bytes()) });
        let pk: [u8; 32] = unb64u(pk).unwrap().try_into().unwrap();
        b64u(&health_proto::hpke::seal(&pk, doc.to_string().as_bytes(), b"sibian-health-1", b"", &[7; 32]).unwrap())
    }
}

struct Device {
    _d: tempfile::TempDir,
    p: Profile,
    session: Mutex<Session>,
}

impl Device {
    fn new() -> Device {
        let d = tempfile::tempdir().unwrap();
        let p = Profile::init(d.path(), "default", &MemoryStore::default()).unwrap();
        Device { _d: d, p, session: Mutex::new(Session::default()) }
    }
    fn net(&self, b: &dyn Backend, config: &Config, f: &mut dyn FnMut(&Net) -> Result<Value>) -> Result<Value> {
        let locked = |g: &mut dyn FnMut(&mut Ctx) -> Result<Value>| -> Result<Value> {
            let mut ctx = Ctx::new(&self.p, NOW, Caller::Owner);
            g(&mut ctx)
        };
        f(&Net { backend: b, config, session: &self.session, locked: &locked, now_ms: NOW, tz_min: 480 })
    }
    fn op(&self, b: &dyn Backend, c: &Config, name: &str, req: Value) -> Result<Value> {
        self.net(b, c, &mut |n| call_net(n, name, req.clone()))
    }
    fn show(&self) -> Value {
        call(&mut Ctx::new(&self.p, NOW, Caller::Owner), "account.show", json!({})).unwrap().value
    }
}

fn config(lab: &Lab) -> Config {
    Config::new(&format!("https://{AUD}"), vec![LabKey { key: lab.did.clone(), may: vec!["lab".into()] }], None)
}

#[test]
fn without_an_account_nothing_is_made_and_nothing_is_sent() {
    let (mock, lab, dev) = (Mock::default(), Lab::new(), Device::new());
    assert_eq!(dev.show()["account"], Value::Null);
    let r = dev.op(&mock, &config(&lab), "inbox.poll", json!({})).unwrap();
    assert_eq!(r["status"], "nokey");
    assert_eq!(dev.op(&mock, &config(&lab), "link.poll", json!({})).unwrap()["state"], "none");
    assert!(mock.0.lock().unwrap().log.is_empty());
    assert_eq!(dev.show()["account"], Value::Null);
}

#[test]
fn registering_shows_the_identity_code_and_the_backend_accepts_the_signature() {
    let (mock, lab, dev) = (Mock::default(), Lab::new(), Device::new());
    let r = dev.op(&mock, &config(&lab), "account.register", json!({})).unwrap();
    let id = r["id"].as_str().unwrap();
    let (uid, pk) = health_proto::codes::read_identity(r["identity"].as_str().unwrap()).unwrap();
    assert_eq!(uid, id);
    assert_eq!(mock.0.lock().unwrap().keys[&b64u(&pk)], id);
    let s = dev.show();
    assert_eq!((s["account"]["id"].as_str(), s["account"]["identity"].clone()), (Some(id), r["identity"].clone()));
    // shown again: already registered, nothing sent
    dev.op(&mock, &config(&lab), "account.register", json!({})).unwrap();
    assert_eq!(mock.0.lock().unwrap().log.len(), 1);
    // nothing secret in what the ops return
    assert!(!s.to_string().contains("amk"));
}

#[test]
fn a_new_device_takes_the_account_only_with_the_check_code_both_show() {
    let (mock, lab) = (Mock::default(), Lab::new());
    let c = config(&lab);
    let (old, new) = (Device::new(), Device::new());
    let reg = old.op(&mock, &c, "account.register", json!({})).unwrap();

    let j = new.op(&mock, &c, "link.join", json!({})).unwrap();
    assert_eq!(new.op(&mock, &c, "link.poll", json!({})).unwrap()["state"], "waiting");
    // rule 2: never without the person's yes
    assert_eq!(old.op(&mock, &c, "link.give", json!({ "code": j["code"], "confirmed": false })).unwrap_err().code, "confirm");
    let given = old.op(&mock, &c, "link.give", json!({ "code": j["code"], "confirmed": true })).unwrap();
    let got = new.op(&mock, &c, "link.poll", json!({})).unwrap();
    assert_eq!((got["state"].as_str(), &got["check"]), (Some("received"), &given["check"]));
    assert_eq!(got["replaces"], false);
    // not stored before it is confirmed
    assert_eq!(new.show()["account"], Value::Null);
    assert!(!new.p.dir.join("vault").exists());
    // the relay deleted it on read: polling again shows the kept state and asks nothing
    let n = mock.0.lock().unwrap().log.len();
    assert_eq!(new.op(&mock, &c, "link.poll", json!({})).unwrap()["check"], given["check"]);
    assert_eq!(mock.0.lock().unwrap().log.len(), n);
    // a wrong check code (or another link's) is refused
    let bad = json!({ "channel": j["channel"], "check": "0000 0000" });
    assert_eq!(new.op(&mock, &c, "link.adopt", bad).unwrap_err().code, "link_changed");
    let adopted = new.op(&mock, &c, "link.adopt", json!({ "channel": j["channel"], "check": got["check"] })).unwrap();
    assert_eq!(adopted["id"], reg["id"]);
    // same account, same identity code once registered again
    assert_eq!(new.show()["account"]["registered"], false);
    let again = new.op(&mock, &c, "account.register", json!({})).unwrap();
    assert_eq!(again["identity"], reg["identity"]);
    assert_eq!(new.op(&mock, &c, "link.poll", json!({})).unwrap()["state"], "none");
}

#[test]
fn someone_else_posting_first_gives_a_different_check_code() {
    let (mock, lab) = (Mock::default(), Lab::new());
    let c = config(&lab);
    let (mine, intruder, new) = (Device::new(), Device::new(), Device::new());
    mine.op(&mock, &c, "account.register", json!({})).unwrap();
    let j = new.op(&mock, &c, "link.join", json!({})).unwrap();
    intruder.op(&mock, &c, "link.give", json!({ "code": j["code"], "confirmed": true })).unwrap();
    // the real device comes second: the relay keeps the first only
    assert_eq!(mine.op(&mock, &c, "link.give", json!({ "code": j["code"], "confirmed": true })).unwrap_err().code, "link_busy");
    let got = new.op(&mock, &c, "link.poll", json!({})).unwrap();
    let mine_check = health_proto::codes::link_check(
        j["channel"].as_str().unwrap(),
        &health_proto::codes::read_link(j["code"].as_str().unwrap()).unwrap().1,
        mine.show()["account"]["id"].as_str().unwrap(),
    );
    assert_ne!(got["check"].as_str().unwrap(), mine_check);
    // not a link code, and an expired link
    assert_eq!(mine.op(&mock, &c, "link.give", json!({ "code": "SBH1:00", "confirmed": true })).unwrap_err().code, "not_link_code");
}

#[test]
fn results_arrive_once_and_bad_ones_are_dropped_and_counted() {
    let (mock, lab, dev) = (Mock::default(), Lab::new(), Device::new());
    let c = config(&lab);
    let reg = dev.op(&mock, &c, "account.register", json!({})).unwrap();
    let id = reg["id"].as_str().unwrap();
    let pk = b64u(&health_proto::codes::read_identity(reg["identity"].as_str().unwrap()).unwrap().1);
    let weight = json!([{ "code": "loinc:29463-7", "v": 68.4, "u": "kg" }]);
    let good = lab.seal(id, "0193a5c2-7e1f-7b3a-9c4d-2f6e8a1b5c90", &pk, weight.clone());
    let unknown = lab.seal(id, "0193a5c2-7e1f-7b3a-9c4d-2f6e8a1b5c91", &pk, json!([{ "code": "x:no_such", "v": 1 }]));
    let someone_else = lab.seal(&"b".repeat(64), "0193a5c2-7e1f-7b3a-9c4d-2f6e8a1b5c92", &pk, weight.clone());
    let rogue = Lab { key: SigningKey::from_bytes(&[8; 32]), did: lab.did.clone() }.seal(
        id,
        "0193a5c2-7e1f-7b3a-9c4d-2f6e8a1b5c93",
        &pk,
        weight.clone(),
    );
    mock.0.lock().unwrap().inbox.insert(
        pk.clone(),
        vec![(1_000, good.clone()), (2_000, unknown), (3_000, someone_else), (4_000, rogue), (5_000, "AAAA".into()), (6_000, good.clone())],
    );
    let r = dev.op(&mock, &c, "inbox.poll", json!({})).unwrap();
    assert_eq!(
        (r["saved"].as_u64(), r["review"].as_u64(), r["seen"].as_u64(), r["dropped"].as_u64()),
        (Some(1), Some(1), Some(1), Some(3))
    );
    assert_eq!(dev.show()["account"]["drops"], 3);
    let mut ctx = Ctx::new(&dev.p, NOW, Caller::Owner);
    let latest = call(&mut ctx, "obs.latest", json!({})).unwrap().value;
    assert_eq!(latest.as_array().unwrap().len(), 1);
    // the cursor moved past all of them
    let n = mock.0.lock().unwrap().log.len();
    let r = dev.op(&mock, &c, "inbox.poll", json!({})).unwrap();
    assert_eq!(r["saved"], 0);
    assert_eq!(mock.0.lock().unwrap().log[n], format!("GET /h/1/inbox/{pk}?after=6000"));
    // the backend forgot the key: registered again, read from the start, the known result is `seen`
    mock.0.lock().unwrap().keys.clear();
    let r = dev.op(&mock, &c, "inbox.poll", json!({})).unwrap();
    assert_eq!((r["saved"].as_u64(), r["seen"].as_u64()), (Some(0), Some(3)), "the saved one, the one in review and its copy");
    // a network failure is an error and moves nothing
    mock.0.lock().unwrap().fail_next = Some("network".into());
    assert_eq!(dev.op(&mock, &c, "inbox.poll", json!({})).unwrap_err().code, "network");
}

#[test]
fn a_self_service_code_is_signed_for_the_registered_key() {
    let (mock, lab, dev) = (Mock::default(), Lab::new(), Device::new());
    let r = dev.op(&mock, &config(&lab), "self.code", json!({ "device": "seca-mbca-01" })).unwrap();
    assert_eq!(r["code"], "160010");
    assert_eq!(dev.op(&mock, &config(&lab), "self.code", json!({ "device": "Bad Id" })).unwrap_err().code, "bad_device");
}

#[test]
fn delegates_reach_none_of_the_account_ops() {
    assert_eq!(health_api::find("account.show").unwrap().delegate, health_api::Delegate::Never);
    // the network ops aren't in the op table at all, so `call` (every delegate's path) can't run them
    for op in health_api::account::NET_OPS {
        assert!(health_api::find(op.name).is_err(), "{}", op.name);
    }
}

#[test]
fn whoop_connects_with_the_persons_own_app_and_syncs_through_the_relay() {
    let (mock, lab, dev) = (Mock::default(), Lab::new(), Device::new());
    let c = config(&lab);
    let wstatus = || call(&mut Ctx::new(&dev.p, NOW, Caller::Owner), "whoop.status", json!({})).unwrap().value;
    assert_eq!(wstatus()["state"], "none");
    assert_eq!(dev.op(&mock, &c, "whoop.sync", json!({})).unwrap_err().code, "whoop_none");
    let b = dev.op(&mock, &c, "whoop.begin", json!({ "client_id": "my-app", "client_secret": "s3cret" })).unwrap();
    let url = b["url"].as_str().unwrap();
    assert!(url.starts_with("https://api.prod.whoop.com/oauth/oauth2/auth?response_type=code&client_id=my-app&redirect_uri=http%3A%2F%2Flocalhost%3A47630%2Fwhoop"));
    let st = url.split("state=").nth(1).unwrap();
    assert_eq!(wstatus()["state"], "pending");
    // a sign-in Health didn't start, and a declined one
    assert_eq!(dev.op(&mock, &c, "whoop.complete", json!({ "query": "code=the-code&state=forged" })).unwrap_err().code, "whoop_state");
    assert_eq!(dev.op(&mock, &c, "whoop.complete", json!({ "query": "error=access_denied" })).unwrap_err().code, "whoop_denied");
    dev.op(&mock, &c, "whoop.complete", json!({ "query": format!("code=the-code&state={st}") })).unwrap();
    assert_eq!(wstatus()["state"], "connected");
    let r = dev.op(&mock, &c, "whoop.sync", json!({ "force": true })).unwrap();
    assert!(r["written"].as_u64().unwrap() > 0, "{r}");
    // under an hour later: skipped; forced: fetched again, nothing new
    assert_eq!(dev.op(&mock, &c, "whoop.sync", json!({})).unwrap()["skipped"], true);
    assert_eq!(dev.op(&mock, &c, "whoop.sync", json!({ "force": true })).unwrap()["written"], 0);
    // the secret went only to the token exchange, never into a URL
    let m = mock.0.lock().unwrap();
    assert!(m.whoop_forms[0].contains("client_secret=s3cret"));
    assert!(!m.log.iter().any(|l| l.contains("s3cret")) && !url.contains("s3cret"));
    drop(m);
    // the refresh token stops working: the person is asked to sign in again
    mock.0.lock().unwrap().whoop_refresh.clear();
    dev.p
        .vault
        .update(|s| {
            s.whoop.as_mut().unwrap().expires = Some(0);
            Ok(())
        })
        .unwrap();
    assert_eq!(dev.op(&mock, &c, "whoop.sync", json!({ "force": true })).unwrap_err().code, "whoop_reauth");
    assert_eq!(wstatus()["state"], "reauth");
    dev.op(&mock, &c, "whoop.disconnect", json!({})).unwrap();
    assert_eq!(wstatus()["state"], "none");
}

#[test]
fn a_mangled_sign_in_reply_is_refused_not_a_crash() {
    let (mock, lab, dev) = (Mock::default(), Lab::new(), Device::new());
    let c = config(&lab);
    dev.op(&mock, &c, "whoop.begin", json!({ "client_id": "a", "client_secret": "b" })).unwrap();
    for q in ["code=%E9%&state=%", "code=%é1&state=x%2", "state=%zz", ""] {
        assert_eq!(dev.op(&mock, &c, "whoop.complete", json!({ "query": q })).unwrap_err().code, "whoop_state", "{q}");
    }
}

/// From the code review: what a link may and may not do after the fact.
#[test]
fn a_received_account_key_expires_and_a_changed_device_is_asked_again() {
    let (mock, lab) = (Mock::default(), Lab::new());
    let c = config(&lab);
    let (old, new) = (Device::new(), Device::new());
    old.op(&mock, &c, "account.register", json!({})).unwrap();
    let j = new.op(&mock, &c, "link.join", json!({})).unwrap();
    old.op(&mock, &c, "link.give", json!({ "code": j["code"], "confirmed": true })).unwrap();
    let got = new.op(&mock, &c, "link.poll", json!({})).unwrap();
    assert_eq!(got["replaces"], false);
    // an account made here after the warning was shown: the adopt is refused, the link forgotten
    new.op(&mock, &c, "account.register", json!({})).unwrap();
    let e = new.op(&mock, &c, "link.adopt", json!({ "channel": j["channel"], "check": got["check"] })).unwrap_err();
    assert_eq!(e.code, "link_changed");
    assert_eq!(new.op(&mock, &c, "link.poll", json!({})).unwrap()["state"], "none");

    // received, then left: ten minutes later the key is gone
    let late = Device::new();
    let j = late.op(&mock, &c, "link.join", json!({})).unwrap();
    old.op(&mock, &c, "link.give", json!({ "code": j["code"], "confirmed": true })).unwrap();
    let got = late.op(&mock, &c, "link.poll", json!({})).unwrap();
    let later = |name: &str, req: Value| {
        late.net(&mock, &c, &mut |n| {
            let n = Net { now_ms: NOW + health_api::account::LINK_MS, ..*n };
            call_net(&n, name, req.clone())
        })
    };
    assert_eq!(later("link.adopt", json!({ "channel": j["channel"], "check": got["check"] })).unwrap_err().code, "link_changed");
    assert_eq!(later("link.poll", json!({})).unwrap()["state"], "none");
    assert_eq!(late.show()["account"], Value::Null);
}

#[test]
fn linking_the_same_account_again_keeps_its_registration() {
    let (mock, lab) = (Mock::default(), Lab::new());
    let c = config(&lab);
    let (old, new) = (Device::new(), Device::new());
    old.op(&mock, &c, "account.register", json!({})).unwrap();
    for _ in 0..2 {
        let j = new.op(&mock, &c, "link.join", json!({})).unwrap();
        old.op(&mock, &c, "link.give", json!({ "code": j["code"], "confirmed": true })).unwrap();
        let got = new.op(&mock, &c, "link.poll", json!({})).unwrap();
        new.op(&mock, &c, "link.adopt", json!({ "channel": j["channel"], "check": got["check"] })).unwrap();
        new.op(&mock, &c, "account.register", json!({})).unwrap();
    }
    let n = mock.0.lock().unwrap().log.iter().filter(|l| l.as_str() == "POST /h/1/key").count();
    assert_eq!(n, 2, "once on each device, not again on the second link");
}

#[test]
fn a_result_with_a_malformed_line_can_still_be_confirmed() {
    let (mock, lab, dev) = (Mock::default(), Lab::new(), Device::new());
    let c = config(&lab);
    let reg = dev.op(&mock, &c, "account.register", json!({})).unwrap();
    let id = reg["id"].as_str().unwrap();
    let pk = b64u(&health_proto::codes::read_identity(reg["identity"].as_str().unwrap()).unwrap().1);
    // "u" is a number, "t" a float, "s" a number: read as the web reads them
    let obs = json!([{ "code": "WT", "v": 70, "u": 5, "t": 1.7951e12, "s": 1 }]);
    mock.0.lock().unwrap().inbox.insert(pk.clone(), vec![(1_000, lab.seal(id, "0193a5c2-7e1f-7b3a-9c4d-2f6e8a1b5c99", &pk, obs))]);
    assert_eq!(dev.op(&mock, &c, "inbox.poll", json!({})).unwrap()["review"], 1);
    let mut ctx = Ctx::new(&dev.p, NOW, Caller::Owner);
    let b = &call(&mut ctx, "review.list", json!({})).unwrap().value[0];
    let r =
        call(&mut ctx, "review.confirm_result", json!({ "id": b["import_id"], "choices": [{ "code": "loinc:29463-7", "unit": "kg" }] }));
    assert_eq!(r.unwrap().value["written"], 1);
}

/// The link history (owner, 2026-11-22: a local record only, list only): what each device records, and when not.
#[test]
fn each_device_records_the_links_it_completed_and_nothing_else() {
    let (mock, lab) = (Mock::default(), Lab::new());
    let c = config(&lab);
    let devices = |d: &Device| call(&mut Ctx::new(&d.p, NOW, Caller::Owner), "link.devices", json!({})).unwrap().value;
    let (old, new, third) = (Device::new(), Device::new(), Device::new());
    assert_eq!(devices(&new), json!({ "state": "ok", "entries": [] }));
    let reg = old.op(&mock, &c, "account.register", json!({})).unwrap();

    // a link that never completes (no confirmation) records nothing on either side
    let j = new.op(&mock, &c, "link.join", json!({})).unwrap();
    let given = old.op(&mock, &c, "link.give", json!({ "code": j["code"], "confirmed": true })).unwrap();
    // the giving side records at once, with the check code it returned
    assert_eq!(given["entry"]["dir"], "gave");
    assert_eq!(given["entry"]["check"], given["check"]);
    assert_eq!(devices(&old)["entries"].as_array().unwrap().len(), 1);
    // a busy relay (someone was first) records nothing
    let e = third.op(&mock, &c, "link.give", json!({ "code": j["code"], "confirmed": true })).err().unwrap();
    assert_eq!(e.code, "link_busy");
    assert_eq!(devices(&third)["entries"], json!([]));
    let got = new.op(&mock, &c, "link.poll", json!({})).unwrap();
    assert_eq!(devices(&new)["entries"], json!([]), "received but not confirmed");

    let adopted = new.op(&mock, &c, "link.adopt", json!({ "channel": j["channel"], "check": got["check"] })).unwrap();
    assert_eq!(adopted["entry"]["dir"], "got");
    let mine = devices(&new)["entries"].clone();
    assert_eq!(mine.as_array().unwrap().len(), 1);
    // both devices recorded the same check code for the same link
    assert_eq!(mine[0]["check"], devices(&old)["entries"][0]["check"]);
    assert_eq!(adopted["id"], reg["id"]);

    // the new device takes yet another account: the first account's entries are no longer shown
    let other = Device::new();
    other.op(&mock, &c, "account.register", json!({})).unwrap();
    let j2 = new.op(&mock, &c, "link.join", json!({})).unwrap();
    other.op(&mock, &c, "link.give", json!({ "code": j2["code"], "confirmed": true })).unwrap();
    let got2 = new.op(&mock, &c, "link.poll", json!({})).unwrap();
    new.op(&mock, &c, "link.adopt", json!({ "channel": j2["channel"], "check": got2["check"] })).unwrap();
    let now_shown = devices(&new)["entries"].clone();
    assert_eq!(now_shown.as_array().unwrap().len(), 1);
    assert_eq!(now_shown[0]["check"], got2["check"]);
}

#[test]
fn an_unreadable_history_is_said_so_and_never_overwritten() {
    let (mock, lab) = (Mock::default(), Lab::new());
    let c = config(&lab);
    let d = Device::new();
    d.op(&mock, &c, "account.register", json!({})).unwrap();
    let path = d.p.dir.join("vault");
    std::fs::write(&path, b"not a sealed vault, but long enough to be read").unwrap();
    let before = std::fs::read(&path).unwrap();
    let v = call(&mut Ctx::new(&d.p, NOW, Caller::Owner), "link.devices", json!({})).unwrap().value;
    assert_eq!(v, json!({ "state": "unreadable" }));
    // a give can't read its account either: it fails before sending, and the file is as it was
    let other = Device::new();
    let j = other.op(&mock, &c, "link.join", json!({})).unwrap();
    assert!(d.op(&mock, &c, "link.give", json!({ "code": j["code"], "confirmed": true })).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(!mock.0.lock().unwrap().log.iter().any(|l| l.starts_with("POST /h/1/link")));
}
