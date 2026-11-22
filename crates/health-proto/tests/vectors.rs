//! The protocol's own test vectors (`B/vectors/health-vectors`, copied to tests/fixtures).

use health_proto::account::Account;
use health_proto::b64::{b64u, base32, unb64u, unbase32};
use health_proto::hpke;
use serde_json::Value;

fn fixture(name: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()).unwrap()
}
fn h(v: &Value) -> Vec<u8> {
    hex::decode(v.as_str().unwrap()).unwrap()
}
fn a32(v: Vec<u8>) -> [u8; 32] {
    v.try_into().unwrap()
}

#[test]
fn signed_requests_match_the_vectors_byte_for_byte() {
    let v = fixture("jws.json");
    let acc = Account::from_sign_seed(&a32(h(&v["account"]["seed"])));
    assert_eq!(b64u(&acc.sign_pub), v["account"]["x"]);
    assert_eq!(acc.id, v["account"]["uid"]);
    for case in ["request", "pass"] {
        let c = &v[case];
        // JSON.stringify of the vector's payload, in its key order
        let payload = serde_json::to_string(&c["payload"]).unwrap();
        let payload = reorder(&payload, &c["payload"]);
        assert_eq!(acc.sign(c["header"]["typ"].as_str().unwrap(), &payload), c["jws"], "{case}");
    }
}

/// serde_json sorts keys; the vectors keep the order JSON.stringify wrote (aud, at, jti, then the body).
fn reorder(_sorted: &str, v: &Value) -> String {
    let order = ["aud", "at", "jti", "pk"];
    let o = v.as_object().unwrap();
    let parts: Vec<String> = order.iter().filter_map(|k| o.get(*k).map(|x| format!("\"{k}\":{x}"))).collect();
    assert_eq!(parts.len(), o.len());
    format!("{{{}}}", parts.join(","))
}

#[test]
fn hpke_matches_rfc_9180_and_the_protocol_vectors() {
    let v = fixture("hpke.json");
    let r = &v["rfc9180"];
    let sealed = hpke::seal(&a32(h(&r["pkRm"])), &h(&r["pt"]), &h(&r["info"]), &h(&r["aad"]), &a32(h(&r["skEm"]))).unwrap();
    assert_eq!(sealed, [h(&r["enc"]), h(&r["ct"])].concat());
    assert_eq!(hpke::open(&a32(h(&r["skRm"])), &sealed, &h(&r["info"]), &h(&r["aad"])).unwrap(), h(&r["pt"]));

    let s = &v["sibian"];
    let info = s["info"].as_str().unwrap().as_bytes();
    let sealed = hpke::seal(&a32(h(&s["pkR"])), &h(&s["pt"]), info, b"", &a32(h(&s["skE"]))).unwrap();
    assert_eq!(sealed, [h(&s["enc"]), h(&s["ct"])].concat());
    assert_eq!(hpke::open(&a32(h(&s["skR"])), &sealed, info, b"").unwrap(), h(&s["pt"]));
    for bad in v["invalid"].as_array().unwrap() {
        let sealed = [h(&bad["enc"]), h(&bad["ct"])].concat();
        // a truncated enc shifts every byte: still refused
        assert!(hpke::open(&a32(h(&bad["skR"])), &sealed, info, b"").is_none(), "{}", bad["name"]);
    }
    // the low-order point (all zeros) is refused
    assert!(hpke::seal(&[0; 32], b"x", info, b"", &[1; 32]).is_none());
}

#[test]
fn base32_and_base64url_have_one_written_form() {
    let pk = unb64u("AQgPFh0kKzI5QEdOVVxjanF4f4aNlJuiqbC3vsXM09o").unwrap();
    assert_eq!(base32(&pk), "AEEA6FQ5EQVTEOKAI5HFKXDDNJYXQ74GRWKJXIVJWC335ROM2PNA");
    assert_eq!(unbase32("AEEA6FQ5EQVTEOKAI5HFKXDDNJYXQ74GRWKJXIVJWC335ROM2PNA", 32).unwrap(), pk);
    assert!(unbase32("AEEA6FQ5EQVTEOKAI5HFKXDDNJYXQ74GRWKJXIVJWC335ROM2PNB", 32).is_none(), "spare bits");
    assert!(unbase32("AEEA6FQ5EQVTEOKAI5HFKXDDNJYXQ74GRWKJXIVJWC335ROM2PN1", 32).is_none());
    assert!(unb64u("AB").is_none() && unb64u("AA").is_some() && unb64u("A+").is_none() && unb64u("A").is_none());
    for n in 0..40 {
        let b: Vec<u8> = (0..n).map(|i| (i * 37 + 11) as u8).collect();
        assert_eq!(unb64u(&b64u(&b)).unwrap(), b);
        assert_eq!(unbase32(&base32(&b), n).unwrap(), b);
    }
}

#[test]
fn ed25519_accepts_and_refuses_exactly_as_libsodium() {
    let v = fixture("ed25519-edges.json");
    for c in v["cases"].as_array().unwrap() {
        let got = health_proto::sig::ed_verify(&h(&c["pub"]), &h(&c["msg"]), &h(&c["sig"]));
        assert_eq!(got, c["ok"].as_bool().unwrap(), "{} / {}", c["from"], c["name"]);
    }
}

#[test]
fn result_files_are_kept_or_dropped_as_the_vectors_say() {
    use health_proto::result::{Context, LabKey, Verdict, check};
    let v = fixture("result.json");
    let ctx = &v["ctx"];
    let keys: Vec<LabKey> = ctx["labKeys"]
        .as_array()
        .unwrap()
        .iter()
        .map(|k| LabKey {
            key: k["key"].as_str().unwrap().into(),
            may: k["may"].as_array().unwrap().iter().map(|m| m.as_str().unwrap().into()).collect(),
        })
        .collect();
    let att: [u8; 32] = unb64u(ctx["att"]["pub"].as_str().unwrap()).unwrap().try_into().unwrap();
    let c = Context { me: ctx["me"].as_str().unwrap(), lab_keys: &keys, att_key: Some(att), iss: ctx["att"]["iss"].as_str().unwrap() };
    for case in v["cases"].as_array().unwrap() {
        let got = check(case["text"].as_str().unwrap().as_bytes(), &c);
        match &case["expect"] {
            Value::String(s) if s == "ok" => assert!(matches!(got, Verdict::Ok(_)), "{}: {got:?}", case["name"]),
            e => assert_eq!(got, Verdict::Drop(Box::leak(e["drop"].as_str().unwrap().to_string().into_boxed_str())), "{}", case["name"]),
        }
    }
    // without the attestation key, a staff result waits
    let staff =
        v["cases"].as_array().unwrap().iter().find(|c| c["expect"] == "ok" && c["text"].as_str().unwrap().contains("staff:")).unwrap();
    let c = Context { att_key: None, ..c };
    assert_eq!(check(staff["text"].as_str().unwrap().as_bytes(), &c), Verdict::Later);
}

#[test]
fn numbers_are_written_as_javascript_writes_them() {
    use health_proto::sig::js_number as j;
    for (x, s) in [
        (68.4, "68.4"),
        (1.0, "1"),
        (712.0, "712"),
        (1e21, "1e+21"),
        (1e20, "100000000000000000000"),
        (1.5e-7, "1.5e-7"),
        (0.000001, "0.000001"),
        (-0.5, "-0.5"),
        (123456.789, "123456.789"),
        (5e-324, "5e-324"),
    ] {
        assert_eq!(j(x), s);
    }
}

#[test]
fn identity_codes_are_read_exactly_as_the_vectors_say() {
    use health_proto::codes::{identity, read_identity};
    let v = fixture("qr.json");
    for ok in v["valid"].as_array().unwrap() {
        let (uid, pk) = read_identity(ok["text"].as_str().unwrap()).unwrap();
        assert_eq!((uid.as_str(), b64u(&pk).as_str()), (ok["uid"].as_str().unwrap(), ok["pk"].as_str().unwrap()));
        assert_eq!(identity(&uid, &pk), ok["text"]);
    }
    for bad in v["invalid"].as_array().unwrap() {
        assert!(read_identity(bad["text"].as_str().unwrap()).is_none(), "{}", bad["name"]);
    }
}

/// What the web code derives, signs and seals from one fixed AMK (scripts/gen-account-fixture.ts).
#[test]
fn the_same_amk_gives_the_web_account_byte_for_byte() {
    use health_proto::account::payload;
    use health_proto::codes::{identity, link, link_check, read_link};
    let w = fixture("web-account.json");
    let amk = a32(h(&w["amk"]));
    let a = Account::of(&amk);
    assert_eq!(a.id, w["id"]);
    assert_eq!(b64u(&a.sign_pub), w["sign_pub"]);
    assert_eq!(b64u(&a.inbox_pub), w["inbox_pub"]);
    assert_eq!(identity(&a.id, &a.inbox_pub), w["identity"]);

    let p = payload(&[("pk", Value::String(b64u(&a.inbox_pub)))], "api-dev-health.4seas.xyz", 1795176000000, &[0x22; 16]);
    assert_eq!(p, w["request"]["payload"]);
    assert_eq!(a.sign("health-key", &p), w["request"]["jws"]);

    let l = &w["link"];
    let sk = a32(h(&l["sk"]));
    let pk = x25519_dalek::PublicKey::from(&x25519_dalek::StaticSecret::from(sk)).to_bytes();
    let channel = l["channel"].as_str().unwrap();
    assert_eq!(link(channel, &pk), l["code"]);
    assert_eq!(read_link(&format!(" {}\n", l["code"].as_str().unwrap())), Some((channel.to_string(), pk)));
    assert_eq!(link_check(channel, &pk, &a.id), l["check"]);
    let plain = l["plain"].as_str().unwrap().as_bytes();
    let sealed = hpke::seal(&pk, plain, b"health-link-1", channel.as_bytes(), &a32(h(&l["eph"]))).unwrap();
    assert_eq!(b64u(&sealed), l["sealed"]);
    assert_eq!(hpke::open(&sk, &sealed, b"health-link-1", channel.as_bytes()).unwrap(), plain);
    assert!(hpke::open(&sk, &sealed, b"health-link-1", b"another channel").is_none());

    let r = &w["result"];
    let sealed = unb64u(r["sealed"].as_str().unwrap()).unwrap();
    assert_eq!(hpke::open(&a.inbox_secret, &sealed, b"sibian-health-1", b"").unwrap(), r["plain"].as_str().unwrap().as_bytes());
}
