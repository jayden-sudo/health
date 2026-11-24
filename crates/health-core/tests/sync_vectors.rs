//! Shared LAN sync vectors (docs/lan-sync.md §6, S5): synthetic profiles whose inventory wire text, digest,
//! fingerprints and subset/part exports the web must reproduce byte for byte. The vectors are committed under
//! tests/fixtures/sync/; `REGEN=1` rewrites them, otherwise this test checks Rust still makes exactly them.
//!
//! Each vector is a plain database (`<name>.db`, the schema of `docs/health-schema.sql`, opened as is: never merged
//! first) plus `vectors.json`: the vault settings and which files are present, and what must come out.

use health_core::correction::{self, Key};
use health_core::db::Db;
use health_core::keys::MemoryStore;
use health_core::profile::Profile;
use health_core::review;
use health_core::sync;

use health_core::vault::{Personal, Stamp, Stopped};
use health_core::write::{Begin, ObsRow, SourceSpec, begin, finish, write};
use health_core::{Value, erase, merge};
use serde_json::{Value as Json, json};
use std::path::{Path, PathBuf};

const T0: i64 = 1_795_000_000_000;
const PACK_PASSWORD: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sync")
}

fn uid(n: u32) -> String {
    format!("0190a000-0000-7000-8000-{n:012}")
}

fn row(metric: &str, start_ms: i64, end_ms: Option<i64>, value: Value, detail: Option<Json>) -> ObsRow {
    ObsRow { metric: metric.into(), start_ms, end_ms, tz_min: 480, value, unit: None, detail }
}

fn done(c: &Db, src: &SourceSpec, n: u32, blob: Option<&str>, rows: &[ObsRow]) -> i64 {
    let (id, _) = begin(c, Begin { source: src, parser: "t", blob, uid: Some(uid(n)), candidates: None }, T0).unwrap();
    write(c, id, rows).unwrap();
    finish(c, id, T0).unwrap();
    id
}

/// One profile with every kind of line and value the inventory writes.
fn build(p: &Profile) {
    let c = &p.db;
    let glu = "loinc:2345-7";
    let hr = "loinc:8867-4";
    let whoop = SourceSpec::new("whoop", "u1");
    // a source whose name and id need byte-length text and UTF-8 byte sorting (a non-BMP character, a newline)
    let odd = SourceSpec::new("lab", "實驗室 \u{1F9EA}\nline2");
    let a = sha('a');
    done(
        c,
        &whoop,
        1,
        None,
        &[
            row(hr, T0 - 3_600_000, None, Value::Int(61), None),
            row(hr, T0 - 3_540_000, Some(T0 - 3_480_000), Value::Real(61.5), Some(json!({"by": "v"}))),
            row(hr, T0 - 3_480_000, None, Value::Real(-0.0), None),
            row(hr, T0 - 3_420_000, None, Value::Real(1e-300), None),
        ],
    );
    // a newer import that takes one of import 1's rows over
    done(c, &whoop, 2, None, &[row(hr, T0 - 3_600_000, None, Value::Int(62), None)]);
    done(c, &odd, 3, Some(&a), &[row(glu, T0 - 86_400_000, None, Value::Real(5.4), Some(json!({"printed": "5.4 mmol/L"})))]);
    // genotypes and report text on a done import
    let g = done(c, &SourceSpec::new("genome", "kit-1"), 4, None, &[]);
    c.execute("INSERT INTO genotype VALUES (?, '1', 12345, 4680, 'AG'), (?, 'MT', 7, NULL, '--')", [g, g]).unwrap();
    c.execute("INSERT INTO report_text (rowid, text) VALUES (?, '未见异常 ok')", [g]).unwrap();
    // a waveform pointer whose file is missing
    c.execute("INSERT INTO metric (code, name, unit, agg) VALUES ('x:test/wave', 'wave', '', 'none')", []).unwrap();
    done(c, &SourceSpec::new("device", "ecg"), 5, None, &[row("x:test/wave", T0 - 7_200_000, None, Value::Text(sha('b')), None)]);
    // a text value and a null value
    c.execute("INSERT INTO metric (code, name, unit, agg) VALUES ('x:test/note', 'note', '', 'none')", []).unwrap();
    done(
        c,
        &SourceSpec::new("manual", ""),
        6,
        None,
        &[
            row("x:test/note", T0 - 600_000, None, Value::Text("a b\nc".into()), None),
            row("x:test/note", T0 - 500_000, None, Value::Null, None),
        ],
    );
    // review (with its file), erased, running
    let ocr = SourceSpec::new("ocr", "photo");
    begin(c, Begin { source: &ocr, parser: "ocr", blob: Some(&sha('c')), uid: Some(uid(7)), candidates: Some(json!({"items": []})) }, T0)
        .unwrap();
    let e = done(c, &whoop, 8, None, &[row(hr, T0 - 10_000_000, None, Value::Int(70), None)]);
    erase::erase(c, e, T0).unwrap();
    begin(c, Begin { source: &whoop, parser: "t", blob: None, uid: Some(uid(9)), candidates: None }, T0).unwrap();
    // corrections: a value and a delete
    let (m, s): (i64, i64) = c
        .query_row(
            "SELECT o.metric_id, o.source_id FROM observation o JOIN metric m ON m.id = o.metric_id WHERE m.code = ? LIMIT 1",
            [hr],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    correction::put(c, Key { metric_id: m, source_id: s, start_ms: T0 - 3_600_000 }, (T0 << 16) + 3, &Value::Real(60.25), T0).unwrap();
    correction::put(c, Key { metric_id: m, source_id: s, start_ms: T0 - 3_420_000 }, (T0 << 16) + 4, &Value::Null, T0).unwrap();
    // a stamped source edit and an alias
    let sid: i64 = c.query_row("SELECT id FROM source WHERE connector = 'whoop'", [], |r| r.get(0)).unwrap();
    health_core::query::source_update(c, sid, Some("My WHOOP"), Some(3), T0, &"d".repeat(32)).unwrap();
    review::add_alias(c, &review::Alias { name: "葡萄糖".into(), unit: "mmol/L".into(), panel: None, code: glu.into() }).unwrap();
    // the file of import 3 is present, the others aren't
    let f = p.tmp().join("a");
    std::fs::write(&f, b"synthetic").unwrap();
    let real = p.blobs.put_file(&f).unwrap();
    c.execute("UPDATE import SET blob = ? WHERE uid = ?", rusqlite_params(&real, &uid(3))).unwrap();
}

fn rusqlite_params<'a>(a: &'a str, b: &'a str) -> [&'a str; 2] {
    [a, b]
}

fn sha(c: char) -> String {
    c.to_string().repeat(64)
}

fn vault(p: &Profile) {
    p.vault
        .update(|s| {
            s.profile = Some(Personal { birth: Some("1988-07".into()), gender: Some("Female".into()), ethnicity: Some("Asian".into()) });
            s.profile_stamp = Some(Stamp { set_at: (T0 << 16) + 9, set_by: "e".repeat(32) });
            s.whoop_stopped = Some(Stopped { at: T0 - 1000, stamp: Stamp { set_at: (T0 << 16) + 2, set_by: "f".repeat(32) } });
            Ok(())
        })
        .unwrap();
}

#[test]
fn shared_sync_vectors() {
    let d = tempfile::tempdir().unwrap();
    let p = Profile::init(d.path(), "v", &MemoryStore::default()).unwrap();
    build(&p);
    vault(&p);
    let inv = sync::of_profile(&p).unwrap();
    let fps: serde_json::Map<String, Json> =
        inv.imports.iter().filter_map(|(u, l)| l.fp.clone().map(|f| (u.clone(), Json::String(f)))).collect();
    let present: Vec<String> = inv.imports.values().filter_map(|l| l.blob.clone()).filter(|b| p.blobs.has(b)).collect();
    // what export_subset and export_part produce, as the inventory of the result
    let tmp = d.path();
    let sub = tmp.join("subset.db");
    merge::export_subset(&p.db, &[uid(1), uid(3), uid(7), uid(8)], true, &sub).unwrap();
    let part1 = tmp.join("part1.db");
    let next = merge::export_part(&p.db, &uid(1), None, 2, &part1).unwrap();
    let part2 = tmp.join("part2.db");
    let last = merge::export_part(&p.db, &uid(1), next.as_ref(), 2, &part2).unwrap();
    let db_inv = |path: &Path| -> String {
        // a plain file's inventory (database lines only: no vault settings, every file present)
        let c = health_core::rusqlite::Connection::open(path).unwrap();
        sync::inventory(&c, &|_| true).unwrap().wire()
    };
    let out = json!({
        "about": "LAN sync shared vectors (docs/lan-sync.md §6). Open each .db as is (it has the health schema). Inventory = database lines plus the vault settings below, files present as listed.",
        "profile": {
            "db": "profile.db",
            "vault": serde_json::to_value(sync::settings_of(&p.vault.load().unwrap())).unwrap(),
            "files_present": present,
            "wire": inv.wire(),
            "digest": inv.digest(),
            "fingerprints": fps,
            "running": inv.running,
        },
        "subset": { "uids": [uid(1), uid(3), uid(7), uid(8)], "corrections": true, "db": "subset.db", "wire": db_inv(&sub) },
        "pack": { "file": "subset.pack", "password": PACK_PASSWORD, "iter": 1, "db_entry_is": "subset.db", "blobs": [] },
        "parts": {
            "uid": uid(1), "max_rows": 2,
            "first": { "db": "part1.db", "next": next, "wire": db_inv(&part1) },
            "second": { "after": next, "db": "part2.db", "next": last, "wire": db_inv(&part2) },
        },
    });
    let text = serde_json::to_string_pretty(&out).unwrap() + "\n";
    let plain = tmp.join("profile.db");
    merge::export_plain(&p.db, &plain).unwrap();
    if std::env::var("REGEN").is_ok() {
        std::fs::create_dir_all(dir()).unwrap();
        std::fs::write(dir().join("vectors.json"), &text).unwrap();
        for (f, name) in [(&plain, "profile.db"), (&sub, "subset.db"), (&part1, "part1.db"), (&part2, "part2.db")] {
            std::fs::copy(f, dir().join(name)).unwrap();
        }
        // a sync pack as a session sends it (iter 1, its salt random, so the bytes differ per run)
        let out = dir().join("subset.pack");
        let _ = std::fs::remove_file(&out);
        health_core::backup::pack_plain(
            &p,
            &dir().join("subset.db"),
            &out,
            PACK_PASSWORD,
            &[],
            T0,
            health_core::pack::Options { iter: 1, ..Default::default() },
        )
        .unwrap();
    }
    let committed = std::fs::read_to_string(dir().join("vectors.json")).expect("run with REGEN=1 once");
    assert_eq!(committed, text, "the vectors changed: rerun with REGEN=1 and send the web the new commit");
    // the committed database gives the committed inventory (what the web does)
    let c = health_core::rusqlite::Connection::open(dir().join("profile.db")).unwrap();
    let mut from_file = sync::inventory(&c, &|b| present.contains(&b.to_string())).unwrap();
    from_file.settings.extend(sync::vault_settings(&p.vault.load().unwrap()));
    from_file.settings.sort();
    assert_eq!(from_file.wire(), inv.wire());
}

#[test]
fn the_committed_sync_pack_restores_the_subset() {
    let d = tempfile::tempdir().unwrap();
    let p = Profile::init(d.path(), "r", &MemoryStore::default()).unwrap();
    health_core::backup::import(&p, &dir().join("subset.pack"), PACK_PASSWORD, T0).unwrap();
    let v: Json = serde_json::from_str(&std::fs::read_to_string(dir().join("vectors.json")).unwrap()).unwrap();
    let mut inv = sync::inventory(&p.db, &|_| true).unwrap();
    inv.settings.retain(|l| l.starts_with("s source "));
    let want = v["subset"]["wire"].as_str().unwrap();
    // merged into a fresh profile: the same imports and corrections as the subset holds
    let lines = |w: &str, tag: &str| w.lines().filter(|l| l.starts_with(tag)).map(String::from).collect::<Vec<_>>();
    assert_eq!(lines(&inv.wire(), "i "), lines(want, "i "));
    assert_eq!(lines(&inv.wire(), "c "), lines(want, "c "));
}
