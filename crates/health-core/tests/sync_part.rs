//! Split imports (docs/lan-sync.md §6.5): a large import travels in parts, each merged as it arrives; the
//! fingerprint shows when the import is complete, and a cursor resumes after an interruption.

use health_core::keys::MemoryStore;
use health_core::profile::Profile;
use health_core::sync;
use health_core::syncstate::Cursor;
use health_core::write::{Begin, ObsRow, SourceSpec, begin, finish, write};
use health_core::{Value, merge};

const T0: i64 = 1_795_000_000_000;

fn big(p: &Profile, n: i64) -> String {
    let src = SourceSpec::new("apple_health", "watch");
    let (id, uid) = begin(&p.db, Begin { source: &src, parser: "t", blob: None, uid: None, candidates: None }, T0).unwrap();
    for (code, step) in [("loinc:8867-4", 60_000), ("loinc:2345-7", 3_600_000)] {
        let rows: Vec<ObsRow> = (0..n)
            .map(|i| ObsRow {
                metric: code.into(),
                start_ms: T0 - 400 * 86_400_000 + i * step,
                end_ms: None,
                tz_min: 480,
                value: Value::Int(60 + i % 40),
                unit: None,
                detail: None,
            })
            .collect();
        write(&p.db, id, &rows).unwrap();
    }
    finish(&p.db, id, T0).unwrap();
    uid
}

fn fp(p: &Profile, uid: &str) -> Option<String> {
    sync::of_profile(p).unwrap().imports.get(uid).and_then(|l| l.fp.clone())
}

#[test]
fn a_large_import_arrives_in_parts_and_resumes_from_its_cursor() {
    let d = tempfile::tempdir().unwrap();
    let keys = MemoryStore::default();
    let (a, b) = (Profile::init(d.path(), "a", &keys).unwrap(), Profile::init(d.path(), "b", &keys).unwrap());
    let uid = big(&a, 600); // 1200 rows over two metrics
    let mut after: Option<Cursor> = None;
    let mut parts = 0;
    loop {
        parts += 1;
        let f = d.path().join(format!("part-{parts}.db"));
        let next = merge::export_part(&a.db, &uid, after.as_ref(), 500, &f).unwrap();
        merge::merge_from(&b.db, &f, T0 + 1).unwrap();
        if next.is_none() {
            break;
        }
        assert_ne!(fp(&b, &uid), fp(&a, &uid), "incomplete after part {parts}");
        // an interruption after part 1: the cursor alone resumes it
        after = next;
        assert!(parts < 10);
    }
    assert_eq!(parts, 3);
    assert_eq!(fp(&b, &uid), fp(&a, &uid));
    // the cursor follows (metric code, start_ms): the first part ends inside the first code's rows
    let f = d.path().join("first.db");
    let c = merge::export_part(&a.db, &uid, None, 500, &f).unwrap().unwrap();
    assert_eq!(c.code, "loinc:2345-7");
    // exactly the rows left: the last part says it's the last
    let f = d.path().join("exact.db");
    let c = merge::export_part(&a.db, &uid, None, 1200, &f).unwrap();
    assert_eq!(c, None);
}
