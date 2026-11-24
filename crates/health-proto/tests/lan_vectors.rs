//! LAN sync signalling vectors (docs/lan-sync.md §4, §5; B/docs/協議.md §3.7) for the web to reproduce byte for
//! byte. `REGEN=1` rewrites tests/fixtures/lan.json; otherwise this checks Rust still makes exactly it.

use health_proto::lan::{Day, days_at, host_only};
use serde_json::json;

const SDP: &str = "v=0\r\na=candidate:1 1 udp 2122260223 192.168.1.5 50000 typ host\r\n\
a=candidate:2 1 udp 2122260223 8.8.8.8 50001 typ host\r\n\
a=candidate:3 1 udp 1686052607 203.0.113.9 50002 typ srflx raddr 192.168.1.5 rport 50000\r\n\
a=candidate:4 1 udp 2122260223 0b1c2d3e-aaaa.local 50003 typ host\r\n\
a=candidate:5 1 udp 2122260223 fe80::1 50004 typ host\r\n\
a=candidate:6 1 udp 2122260223 10.0.0.7 50005 typ host\r\n\
a=candidate:7 1 udp 2122260223 172.32.0.1 50006 typ host\r\na=end\r\n";

#[test]
fn lan_signalling_vectors() {
    let amk = [7u8; 32];
    let d = Day::of(&amk, "2026-11-18");
    // the exact bytes sealed (no key-order question)
    let plain = r#"{"v":1,"t":1795000000000,"id":"AAECAwQFBgcICQoLDA0ODw","from":"0123456789abcdef0123456789abcdef","type":"hello","name":"Health 0.1 · macos","proto":[1]}"#;
    let ct = d.seal(plain.as_bytes(), [1; 12]);
    assert_eq!(d.open(&ct).unwrap(), plain.as_bytes());
    let midnight: i64 = 1_795_046_400_000; // 2026-11-19T00:00Z
    let days: Vec<_> = [midnight + 5 * 60_000, midnight - 60_000, midnight + 3_600_000, 0, 951_782_400_000]
        .iter()
        .map(|t| json!({ "now_ms": t, "days": days_at(*t) }))
        .collect();
    let out = json!({
        "about": "LAN sync signalling (health-proto/src/lan.rs). HKDF-SHA256(salt 'health-amk-v1', amk), info 'lan-sync/key/v1/<day>' and 'lan-sync/rid/v1/<day>'; rid = base32 lower case, no padding, of the first 16 bytes; ct = base64url(nonce ‖ AES-256-GCM(key, nonce, plain, aad = rid ASCII)).",
        "amk_hex": hex::encode(amk),
        "day": d.day,
        "key_hex": hex::encode(*d.key),
        "rid": d.rid,
        "next_day_rid": Day::of(&amk, "2026-11-19").rid,
        "seal": { "nonce_hex": hex::encode([1u8; 12]), "plain": plain, "ct": ct },
        "days_at": days,
        "host_only": { "sdp": SDP, "kept": host_only(SDP) },
    });
    let text = serde_json::to_string_pretty(&out).unwrap() + "\n";
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/lan.json");
    if std::env::var("REGEN").is_ok() {
        std::fs::write(&path, &text).unwrap();
    }
    assert_eq!(std::fs::read_to_string(&path).expect("REGEN=1 once"), text);
}
