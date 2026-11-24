//! Regressions from the independent review of S1 (docs/merge-fixes.md §8b): each test reproduced a hole found then.
//! Written by the reviewer as probes; kept here so the fixes stay fixed.

use health_core::db::Db;
use health_core::keys::MemoryStore;
use health_core::manual::{self, Entry, When};
use health_core::profile::Profile;
use health_core::sync::{self, Inventory};
use health_core::write::{Begin, ObsRow, SourceSpec, begin, finish, write};
use health_core::{Value, erase, merge, query};
use std::path::Path;

const T0: i64 = 1_795_000_000_000;
const GLU: &str = "loinc:2345-7";
const RHR: &str = "loinc:40443-4";
const WEIGHT: &str = "loinc:29463-7";
const X: i64 = T0 - 3_600_000;

fn inv(p: &Profile) -> Inventory {
    sync::inventory(&p.db, &|_| true).unwrap()
}
fn pull(x: &Profile, y: &Profile, dir: &Path, now: i64, n: &mut u32) {
    let w = sync::wants(&inv(x), &inv(y), &|_| false);
    if w.is_empty() {
        return;
    }
    *n += 1;
    let f = dir.join(format!("subset-{n}.db"));
    let _ = std::fs::remove_file(&f);
    merge::export_subset(&y.db, &w.imports, w.corrections, &f).unwrap();
    merge::merge_from(&x.db, &f, now).unwrap();
}
fn exchange(x: &Profile, y: &Profile, dir: &Path, now: i64, n: &mut u32) {
    let (ix, iy) = (inv(x), inv(y));
    let (wx, wy) = (sync::wants(&ix, &iy, &|_| false), sync::wants(&iy, &ix, &|_| false));
    let mut files = Vec::new();
    for (w, from) in [(&wx, y), (&wy, x)] {
        *n += 1;
        let f = dir.join(format!("subset-{n}.db"));
        let _ = std::fs::remove_file(&f);
        if !w.is_empty() {
            merge::export_subset(&from.db, &w.imports, w.corrections, &f).unwrap();
            files.push(Some(f));
        } else {
            files.push(None);
        }
    }
    for (to, f) in [(x, &files[0]), (y, &files[1])] {
        if let Some(f) = f {
            merge::merge_from(&to.db, f, now).unwrap();
        }
    }
}
/// Returns whether digests agree within `max` rounds.
fn converge(devs: &[&Profile], dir: &Path, now: &mut i64, max: u32) -> bool {
    let mut n = 1000;
    for round in 0.. {
        let d: Vec<String> = devs.iter().map(|p| inv(p).digest()).collect();
        if d.iter().all(|x| *x == d[0]) {
            return true;
        }
        if round >= max {
            return false;
        }
        for x in 0..devs.len() {
            for y in x + 1..devs.len() {
                *now += 1_000;
                exchange(devs[x], devs[y], dir, *now, &mut n);
            }
        }
    }
    unreachable!()
}
fn devs(n: usize) -> (tempfile::TempDir, MemoryStore, Vec<Profile>) {
    let dir = tempfile::tempdir().unwrap();
    let keys = MemoryStore::default();
    let v = (0..n).map(|i| Profile::init(dir.path(), &format!("d{i}"), &keys).unwrap()).collect();
    (dir, keys, v)
}
fn import(c: &Db, src: &SourceSpec, metric: &str, rows: &[(i64, Value)], now: i64) -> i64 {
    let (id, _) = begin(c, Begin { source: src, parser: "t", blob: None, uid: None, candidates: None }, now).unwrap();
    let rows: Vec<ObsRow> = rows
        .iter()
        .map(|(t, v)| ObsRow {
            metric: metric.into(),
            start_ms: *t,
            end_ms: None,
            tz_min: 480,
            value: v.clone(),
            unit: None,
            detail: Some(serde_json::json!({"k": 1})),
        })
        .collect();
    write(c, id, &rows).unwrap();
    finish(c, id, now).unwrap();
    id
}
fn val(c: &Db, code: &str, t: i64) -> Option<(Value, String)> {
    c.query_row(
        "SELECT o.value, i.uid FROM observation o JOIN metric m ON m.id = o.metric_id JOIN import i ON i.id = o.import_id WHERE m.code = ? AND o.start_ms = ?",
        rusqlite::params![code, t],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .ok()
}
fn whoop() -> SourceSpec {
    SourceSpec::new("whoop", "u1")
}

#[test]
fn p1_lab_uid_takes_over_rows_of_other_sources() {
    let (dir, _k, d) = devs(2);
    let (a, b) = (&d[0], &d[1]);
    import(&a.db, &whoop(), RHR, &[(X, Value::Int(60))], T0);
    let wi = import(&b.db, &whoop(), RHR, &[(X, Value::Int(999))], T0 + 1);
    import(&b.db, &SourceSpec::new("lab", "evil"), GLU, &[(X, Value::Int(5))], T0 + 2);
    let f = dir.path().join("evil.db");
    merge::export_plain(&b.db, &f).unwrap();
    let t = rusqlite::Connection::open(&f).unwrap();
    t.execute("UPDATE import SET uid = 'zzzz', source_id = (SELECT id FROM source WHERE connector = 'lab') WHERE id = ?", [wi]).unwrap();
    drop(t);
    let rep = merge::merge_from(&a.db, &f, T0 + 10).unwrap();
    println!("P1 report: refused_imports={} added={}", rep.refused_imports, rep.added_imports);
    println!("P1 after hostile merge: {:?}", val(&a.db, RHR, X));
    // a later genuine local import of the same point
    import(&a.db, &whoop(), RHR, &[(X, Value::Int(61))], T0 + 86_400_000 * 30);
    let v = val(&a.db, RHR, X);
    println!("P1 after a newer local whoop import: {v:?}");
    assert_eq!(v.map(|x| x.0), Some(Value::Int(61)), "a non-UUID uid on a lab source owns a whoop row for good");
}

#[test]
fn p2_rows_outside_the_span() {
    let (dir, _k, d) = devs(2);
    let (a, b) = (&d[0], &d[1]);
    let mut now = T0;
    import(&a.db, &whoop(), RHR, &[(X, Value::Int(60))], now);
    assert!(converge(&[a, b], dir.path(), &mut now, 4));
    let f = dir.path().join("span.db");
    merge::export_plain(&a.db, &f).unwrap();
    let t = rusqlite::Connection::open(&f).unwrap();
    t.execute(
        "INSERT INTO observation (metric_id, start_ms, source_id, end_ms, tz_min, value, import_id, detail)
         SELECT metric_id, start_ms + 864000000, source_id, NULL, tz_min, 77, import_id, NULL FROM observation LIMIT 1",
        [],
    )
    .unwrap();
    drop(t);
    merge::merge_from(&b.db, &f, now).unwrap();
    println!("P2 b holds the stray row: {:?}", val(&b.db, RHR, X + 864_000_000));
    let ok = converge(&[a, b], dir.path(), &mut now, 6);
    println!("P2 converged after 6 rounds: {ok}; a has the row: {:?}", val(&a.db, RHR, X + 864_000_000));
    let id: i64 = b.db.query_row("SELECT id FROM import WHERE status = 'done'", [], |r| r.get(0)).unwrap();
    let e = erase::erase(&b.db, id, now);
    println!("P2 erase on b: {:?}", e.as_ref().map(|r| r.deleted).map_err(|e| format!("{e:?}")));
    // the tombstone from a
    let ida: i64 = a.db.query_row("SELECT id FROM import WHERE status = 'done'", [], |r| r.get(0)).unwrap();
    erase::erase(&a.db, ida, now).unwrap();
    let f2 = dir.path().join("tomb.db");
    merge::export_plain(&a.db, &f2).unwrap();
    let m = merge::merge_from(&b.db, &f2, now);
    println!("P2 merging a's tombstone into b: {:?}", m.as_ref().map(|r| r.erased_imports).map_err(|e| format!("{e:?}")));
    assert!(ok && e.is_ok() && m.is_ok());
}

#[test]
fn p3_invalid_utf8_from_a_pack() {
    let (dir, _k, d) = devs(2);
    let (a, b) = (&d[0], &d[1]);
    import(&b.db, &whoop(), RHR, &[(X, Value::Int(60))], T0);
    let f = dir.path().join("utf8.db");
    merge::export_plain(&b.db, &f).unwrap();
    let t = rusqlite::Connection::open(&f).unwrap();
    t.execute("UPDATE observation SET detail = CAST(x'ff' AS TEXT)", []).unwrap();
    drop(t);
    let m = merge::merge_from(&a.db, &f, T0 + 10);
    println!("P3 merge: {:?}", m.as_ref().map(|r| r.added_imports).map_err(|e| format!("{e:?}")));
    let i = sync::inventory(&a.db, &|_| true);
    println!("P3 inventory after: {:?}", i.as_ref().map(|_| "ok").map_err(|e| format!("{e:?}")));
    assert!(m.is_err() || i.is_ok(), "the merge took a row that makes every later inventory fail");
}

#[test]
fn p4_review_photo_under_r6() {
    let (dir, _k, d) = devs(3);
    let (a, b, c) = (&d[0], &d[1], &d[2]);
    let mut now = T0;
    let src = SourceSpec::new("ocr", "photo");
    let sha = "e".repeat(64);
    // c imported the file first (smaller uid), done
    let (id, _) = begin(&c.db, Begin { source: &src, parser: "ocr", blob: Some(&sha), uid: None, candidates: None }, now - 10_000).unwrap();
    write(
        &c.db,
        id,
        &[ObsRow { metric: GLU.into(), start_ms: X, end_ms: None, tz_min: 480, value: Value::Int(90), unit: None, detail: None }],
    )
    .unwrap();
    finish(&c.db, id, now - 10_000).unwrap();
    // a holds the same file as a review, synced to b
    begin(
        &a.db,
        Begin { source: &src, parser: "ocr", blob: Some(&sha), uid: None, candidates: Some(serde_json::json!({"items": []})) },
        now,
    )
    .unwrap();
    assert!(converge(&[a, b], dir.path(), &mut now, 4));
    let mut n = 0;
    pull(a, c, dir.path(), now, &mut n);
    let q = |p: &Profile| -> Vec<(String, Option<String>)> {
        p.db.prepare("SELECT status, blob FROM import ORDER BY uid")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    };
    println!("P4 a after merging c: {:?}", q(a));
    let u1: i64 = a.db.query_row("SELECT id FROM import WHERE status = 'done'", [], |r| r.get(0)).unwrap();
    let e = erase::erase(&a.db, u1, now).unwrap();
    println!("P4 a erases u1: orphan_blobs={:?} (the pending review's photo)", e.orphan_blobs);
    let ok = converge(&[a, b], dir.path(), &mut now, 6);
    println!("P4 a: {:?}\nP4 b: {:?}", q(a), q(b));
    println!(
        "P4 wants a<-b empty: {}, b<-a empty: {}",
        sync::wants(&inv(a), &inv(b), &|_| false).is_empty(),
        sync::wants(&inv(b), &inv(a), &|_| false).is_empty()
    );
    assert!(e.orphan_blobs.is_empty(), "the file of a review still pending was deleted");
    assert!(ok, "digests never agree and nothing is wanted");
}

#[test]
fn p6_reopen_stamps_and_beats_a_real_edit() {
    let dir = tempfile::tempdir().unwrap();
    let keys = MemoryStore::default();
    let a = Profile::init(dir.path(), "a", &keys).unwrap();
    let b = Profile::init(dir.path(), "b", &keys).unwrap();
    let mut now = T0;
    import(&a.db, &whoop(), RHR, &[(X, Value::Int(60))], now);
    assert!(converge(&[&a, &b], dir.path(), &mut now, 4));
    // b restores an old backup in which the source had priority 200, unstamped (tail rule: 200 > 100)
    let f = dir.path().join("old.db");
    merge::export_plain(&b.db, &f).unwrap();
    let t = rusqlite::Connection::open(&f).unwrap();
    t.execute("UPDATE source SET priority = 200 WHERE connector = 'whoop'", []).unwrap();
    drop(t);
    merge::merge_from(&b.db, &f, now).unwrap();
    // the person then chooses 50 on a
    now += 60_000;
    let sid: i64 = a.db.query_row("SELECT id FROM source WHERE connector = 'whoop'", [], |r| r.get(0)).unwrap();
    query::source_update(&a.db, sid, None, Some(50), now, "a").unwrap();
    // b restarts later, before syncing
    drop(b);
    now += 60_000;
    let b = Profile::open(dir.path(), "b", &keys, true, now).unwrap();
    assert!(converge(&[&a, &b], dir.path(), &mut now, 4));
    let p = |x: &Profile| -> (i64, Option<String>) {
        x.db.query_row("SELECT priority, meta FROM source WHERE connector = 'whoop'", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap()
    };
    println!("P6 a: {:?} b: {:?}", p(&a), p(&b));
    assert_eq!(p(&a).0, 50, "the person's edit lost to a stamp b made by restarting");
}

#[test]
fn p7_removed_manual_entry_comes_back() {
    let (dir, _k, d) = devs(3);
    let (a, b, c) = (&d[0], &d[1], &d[2]);
    let mut now = T0;
    let e = |v: i64| Entry {
        metric: WEIGHT.into(),
        value: Value::Int(v),
        unit: Some("kg".into()),
        at: When::At { ms: X, end_ms: None, tz_min: 480 },
    };
    manual::enter(&a.db, &[e(71)], now).unwrap();
    assert!(converge(&[a, c], dir.path(), &mut now, 4));
    now += 1_000;
    manual::enter(&b.db, &[e(72)], now).unwrap(); // the same day's weight typed on another device
    assert!(converge(&[a, b], dir.path(), &mut now, 4));
    println!("P7 a and b after sync: {:?}", val(&a.db, WEIGHT, X));
    now += 1_000;
    manual::remove(&a.db, WEIGHT, X, now).unwrap();
    assert!(converge(&[a, b], dir.path(), &mut now, 4));
    println!("P7 after remove on a, synced with b: a={:?} b={:?}", val(&a.db, WEIGHT, X), val(&b.db, WEIGHT, X));
    assert!(converge(&[a, b, c], dir.path(), &mut now, 6));
    println!("P7 after c joins: a={:?} b={:?} c={:?}", val(&a.db, WEIGHT, X), val(&b.db, WEIGHT, X), val(&c.db, WEIGHT, X));
    assert_eq!(val(&a.db, WEIGHT, X), None, "the removed entry returned with its older value");
}

#[test]
fn p8_running_import_is_never_wanted() {
    let (dir, _k, d) = devs(2);
    let (a, b) = (&d[0], &d[1]);
    let src = SourceSpec::new("lab", "dev1");
    let (id, uid) =
        begin(&a.db, Begin { source: &src, parser: "t", blob: None, uid: Some("result-1".into()), candidates: None }, T0).unwrap();
    write(
        &a.db,
        id,
        &[ObsRow { metric: GLU.into(), start_ms: X, end_ms: None, tz_min: 480, value: Value::Int(5), unit: None, detail: None }],
    )
    .unwrap();
    finish(&a.db, id, T0).unwrap();
    begin(&b.db, Begin { source: &src, parser: "t", blob: None, uid: Some(uid.clone()), candidates: None }, T0).unwrap(); // running on b
    let mut n = 0;
    for round in 0..3 {
        let w = sync::wants(&inv(b), &inv(a), &|_| false);
        println!("P8 round {round}: b wants {:?}", w.imports);
        pull(b, a, dir.path(), T0 + 1000, &mut n);
    }
    assert!(sync::wants(&inv(b), &inv(a), &|_| false).imports.is_empty(), "spec 6.4: running is never wanted");
}

#[test]
fn p9_corrections_equal_at_integer_vs_real() {
    let (dir, _k, d) = devs(2);
    let (a, b) = (&d[0], &d[1]);
    let mut now = T0;
    import(&a.db, &whoop(), RHR, &[(X, Value::Int(60))], now);
    assert!(converge(&[a, b], dir.path(), &mut now, 4));
    let at = (now << 16) + 1;
    for (p, v) in [(a, "5"), (b, "5.0")] {
        p.db.execute(&format!("INSERT INTO correction SELECT metric_id, source_id, start_ms, {at}, {v} FROM observation"), []).unwrap();
    }
    let ok = converge(&[a, b], dir.path(), &mut now, 6);
    println!("P9 a: {:?}\nP9 b: {:?}", inv(a).corrections, inv(b).corrections);
    assert!(ok, "equal at, 5 against 5.0: neither wins, digests differ for good");
}

#[test]
fn p10_a_uid_that_could_forge_inventory_lines_is_refused() {
    // merge-fixes P5: uids are 1-64 bytes in 0x21-0x7E, locally and from packs
    let (_dir, _k, d) = devs(1);
    let a = &d[0];
    let src = SourceSpec::new("lab", "dev1");
    let fake = "00000000-0000-7000-8000-000000000001 erased - -\ni zz";
    assert!(begin(&a.db, Begin { source: &src, parser: "t", blob: None, uid: Some(fake.into()), candidates: None }, T0).is_err());
    assert_eq!(inv(a).text().lines().filter(|l| l.starts_with("i ")).count(), 0);
}
