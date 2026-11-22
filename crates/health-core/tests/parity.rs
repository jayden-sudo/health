//! Web parity (plan M1 acceptance): a backup made by the web code merges into a fresh Rust database
//! and gives the same v_daily and v_observation the web computed. Fixture: scripts/web-fixture.ts.

use health_core::{db, merge, pack};
use serde_json::{Value, json};
use std::collections::BTreeMap;

const FIX: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

fn unpack_db(dir: &std::path::Path) -> std::path::PathBuf {
    let bytes = std::fs::read(format!("{FIX}/web-backup.zuhealth")).unwrap();
    let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    pack::read(&bytes[..], "fixture-pass", |n, b, _| {
        files.entry(n.into()).or_default().extend_from_slice(b);
        Ok(())
    })
    .unwrap();
    let p = dir.join("plain.db");
    std::fs::write(&p, &files["db.sqlite"]).unwrap();
    p
}

fn rows(c: &db::Db, sql: &str) -> Vec<Value> {
    let mut st = c.prepare(sql).unwrap();
    let names: Vec<String> = st.column_names().map(|s| s.to_string()).collect();
    st.query_map([], |r| {
        let mut o = serde_json::Map::new();
        for (i, n) in names.iter().enumerate() {
            let v: health_core::Value = r.get(i)?;
            o.insert(n.clone(), serde_json::to_value(v).unwrap());
        }
        Ok(Value::Object(o))
    })
    .unwrap()
    .collect::<Result<_, _>>()
    .unwrap()
}

/// Numbers compared as f64 within 1e-9 relative; everything else exactly.
fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            let (x, y) = (x.as_f64().unwrap(), y.as_f64().unwrap());
            (x - y).abs() <= 1e-9 * x.abs().max(1.0)
        }
        (Value::Object(x), Value::Object(y)) => x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same(v, w))),
        (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(v, w)| same(v, w)),
        _ => a == b,
    }
}

#[test]
fn web_backup_merges_to_the_same_results() {
    let tmp = tempfile::tempdir().unwrap();
    let plain = unpack_db(tmp.path());
    let expected: Value = serde_json::from_str(&std::fs::read_to_string(format!("{FIX}/web-expected.json")).unwrap()).unwrap();
    let now = expected["now"].as_i64().unwrap();

    let c = db::open_memory().unwrap();
    let rep = merge::merge_from(&c, &plain, now).unwrap();
    assert!(rep.skipped_metrics.is_empty(), "{:?}", rep.skipped_metrics);

    let daily = rows(
        &c,
        "SELECT m.code, d.day, s.connector, s.external_id, d.n, d.sum, d.min, d.max, d.seconds
           FROM v_daily d JOIN metric m ON m.id = d.metric_id JOIN source s ON s.id = d.source_id ORDER BY m.code, d.day",
    );
    let obs = rows(
        &c,
        "SELECT m.code, o.start_ms, s.connector, s.external_id, o.end_ms, o.tz_min, o.value, i.uid, o.detail, o.corrected
           FROM v_observation o JOIN metric m ON m.id = o.metric_id JOIN source s ON s.id = o.source_id JOIN import i ON i.id = o.import_id
          ORDER BY m.code, o.start_ms, s.connector, s.external_id",
    );
    let imports = rows(&c, "SELECT uid, status, parser FROM import ORDER BY uid");
    for (name, got) in [("daily", json!(daily)), ("obs", json!(obs)), ("imports", json!(imports))] {
        assert!(same(&got, &expected[name]), "{name} differs\n got: {got:#}\nwant: {:#}", expected[name]);
    }
    // the pending review stays pending and invisible; the report text came along
    assert_eq!(rep.needed_blobs.len(), 3);
    let hits: i64 = c.query_row("SELECT count(*) FROM report_text WHERE report_text MATCH '未見異常'", [], |r| r.get(0)).unwrap();
    assert_eq!(hits, 1);
}

#[test]
fn merging_the_same_backup_twice_changes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let plain = unpack_db(tmp.path());
    let c = db::open_memory().unwrap();
    merge::merge_from(&c, &plain, 0).unwrap();
    let snap = |c: &db::Db| {
        (
            rows(c, "SELECT * FROM v_observation ORDER BY 1, 2, 3"),
            rows(c, "SELECT * FROM daily ORDER BY 1, 2, 3"),
            rows(c, "SELECT uid, status FROM import ORDER BY uid"),
        )
    };
    let before = snap(&c);
    let rep = merge::merge_from(&c, &plain, 0).unwrap();
    assert_eq!(rep.added_imports, 0);
    assert_eq!(before, snap(&c));
}

#[test]
fn rejects_a_plain_db_with_extra_objects() {
    let tmp = tempfile::tempdir().unwrap();
    let plain = unpack_db(tmp.path());
    rusqlite::Connection::open(&plain).unwrap().execute_batch("CREATE TRIGGER evil AFTER INSERT ON metric BEGIN SELECT 1; END;").unwrap();
    let c = db::open_memory().unwrap();
    assert_eq!(merge::merge_from(&c, &plain, 0).unwrap_err().code, "backup_db");
}
