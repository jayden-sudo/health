//! Sync core (docs/lan-sync.md §6, S1): three devices make random changes and sync pairwise by inventory and subset
//! packs, in random orders, until their digests agree. Then their tables must be identical by identity, `daily` must
//! equal a full rebuild, and a full-database merge between them must change nothing (the subsets missed nothing).

use health_core::db::Db;
use health_core::keys::MemoryStore;
use health_core::profile::Profile;
use health_core::review;
use health_core::sync::{self, Inventory};
use health_core::write::{Begin, ObsRow, SourceSpec, begin, finish, write};
use health_core::{Value, daily, erase, merge};
use std::path::Path;

#[path = "common/steps.rs"]
mod steps;
use steps::*;

fn inv(p: &Profile) -> Inventory {
    sync::inventory(&p.db, &|_| true).unwrap()
}

/// x pulls what it wants from y through a subset pack.
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

fn run(seed: u64) {
    let dir = tempfile::tempdir().unwrap();
    let keys = MemoryStore::default();
    let devs: Vec<Profile> = (0..3).map(|i| Profile::init(dir.path(), &format!("d{i}"), &keys).unwrap()).collect();
    let mut r = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let mut now = T0;
    let mut packs = 0;
    // changes interleaved with partial syncs, so devices diverge and meet in different orders
    for _ in 0..40 {
        now += 1_000;
        let d = r.below(3) as usize;
        step(&devs[d], d, &mut r, now);
        if r.below(4) == 0 {
            let (x, y) = (r.below(3) as usize, r.below(3) as usize);
            if x != y {
                if r.below(2) == 0 {
                    pull(&devs[x], &devs[y], dir.path(), now, &mut packs);
                } else {
                    exchange(&devs[x], &devs[y], dir.path(), now, &mut packs);
                }
            }
        }
    }
    // sync until every digest agrees
    let mut rounds = 0;
    loop {
        let digests: Vec<String> = devs.iter().map(|p| inv(p).digest()).collect();
        if digests.iter().all(|x| *x == digests[0]) {
            break;
        }
        rounds += 1;
        assert!(rounds <= 8, "seed {seed}: no convergence after 8 rounds");
        let mut pairs: Vec<(usize, usize)> = (0..3).flat_map(|x| (0..3).filter(move |y| *y != x).map(move |y| (x, y))).collect();
        for i in (1..pairs.len()).rev() {
            pairs.swap(i, r.below(i as u64 + 1) as usize);
        }
        for (x, y) in pairs {
            now += 1_000;
            if x < y {
                exchange(&devs[x], &devs[y], dir.path(), now, &mut packs);
            } else {
                pull(&devs[x], &devs[y], dir.path(), now, &mut packs);
            }
        }
    }
    let first = dump(&devs[0].db);
    for (i, p) in devs.iter().enumerate() {
        assert_eq!(dump(&p.db), first, "seed {seed}: device {i} differs from device 0");
        let before = daily_text(&p.db);
        daily::rebuild_all(&p.db, now).unwrap();
        assert_eq!(daily_text(&p.db), before, "seed {seed}: device {i}'s daily differs from a full rebuild");
    }
    // a full-database merge finds nothing the subsets missed
    for (x, y) in [(0, 1), (1, 2), (2, 0)] {
        let f = dir.path().join(format!("full-{x}-{y}.db"));
        merge::export_plain(&devs[y].db, &f).unwrap();
        merge::merge_from(&devs[x].db, &f, now).unwrap();
        assert_eq!(dump(&devs[x].db), first, "seed {seed}: a full merge {x} ← {y} changed something");
    }
}

#[test]
fn three_devices_converge_by_subset_sync() {
    // SEEDS=1200 for a deep run; 263, 482 and 1070 failed before merge-fixes R6b and stay in every run
    let seeds: u64 = std::env::var("SEEDS").ok().and_then(|s| s.parse().ok()).unwrap_or(40);
    for seed in (1..=seeds).chain([263, 482, 1070]) {
        run(seed);
    }
}

#[test]
fn equal_data_gives_equal_digests_and_an_empty_want() {
    let dir = tempfile::tempdir().unwrap();
    let keys = MemoryStore::default();
    let (a, b) = (Profile::init(dir.path(), "a", &keys).unwrap(), Profile::init(dir.path(), "b", &keys).unwrap());
    assert_eq!(inv(&a).digest(), inv(&b).digest());
    assert!(sync::wants(&inv(&a), &inv(&b), &|_| false).is_empty());
}

/// Both sides pull at once, as on a channel (§6.3): each exports what the other wants before either merges.
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

/// Exchanges between every pair until every digest agrees; panics after `max` rounds.
fn converge(devs: &[&Profile], dir: &Path, now: &mut i64, max: u32) {
    let mut n = 0;
    for round in 0.. {
        let d: Vec<String> = devs.iter().map(|p| inv(p).digest()).collect();
        if d.iter().all(|x| *x == d[0]) {
            return;
        }
        assert!(round < max, "no convergence after {max} rounds");
        for x in 0..devs.len() {
            for y in x + 1..devs.len() {
                *now += 1_000;
                exchange(devs[x], devs[y], dir, *now, &mut n);
            }
        }
    }
}

fn two() -> (tempfile::TempDir, Profile, Profile) {
    let dir = tempfile::tempdir().unwrap();
    let keys = MemoryStore::default();
    let a = Profile::init(dir.path(), "a", &keys).unwrap();
    let b = Profile::init(dir.path(), "b", &keys).unwrap();
    (dir, a, b)
}

fn import_rows(c: &Db, rows: &[(i64, Value)], now: i64) -> i64 {
    let src = SourceSpec::new("whoop", "u1");
    let (id, _) = begin(c, Begin { source: &src, parser: "t", blob: None, uid: None, candidates: None }, now).unwrap();
    let rows: Vec<ObsRow> = rows
        .iter()
        .map(|(t, v)| ObsRow { metric: RHR.into(), start_ms: *t, end_ms: None, tz_min: 480, value: v.clone(), unit: None, detail: None })
        .collect();
    write(c, id, &rows).unwrap();
    finish(c, id, now).unwrap();
    id
}

fn rhr(c: &Db, t: i64) -> Option<Value> {
    c.query_row(
        "SELECT o.value FROM observation o JOIN metric m ON m.id = o.metric_id WHERE m.code = ? AND o.start_ms = ?",
        rusqlite::params![RHR, t],
        |r| r.get(0),
    )
    .ok()
}

#[test]
fn erasing_an_import_that_took_rows_over_converges_through_subsets() {
    // review finding 1, through inventory + subset packs rather than a full merge
    let (dir, a, b) = two();
    let mut now = T0;
    let x = T0 - 3_600_000;
    import_rows(&a.db, &[(x, Value::Int(60))], now);
    converge(&[&a, &b], dir.path(), &mut now, 4);
    now += 1_000;
    let newer = import_rows(&a.db, &[(x, Value::Int(70))], now);
    erase::erase(&a.db, newer, now).unwrap();
    assert_eq!(rhr(&a.db, x), None, "a lost the row with the erase");
    converge(&[&a, &b], dir.path(), &mut now, 4);
    assert_eq!((rhr(&a.db, x), rhr(&b.db, x)), (Some(Value::Int(60)), Some(Value::Int(60))));
}

#[test]
fn a_review_confirmed_differently_on_two_devices_converges_through_subsets() {
    let (dir, a, b) = two();
    let mut now = T0;
    let src = SourceSpec::new("ocr", "photo");
    let sha = "e".repeat(64);
    begin(
        &a.db,
        Begin { source: &src, parser: "ocr", blob: Some(&sha), uid: None, candidates: Some(serde_json::json!({"items": []})) },
        now,
    )
    .unwrap();
    converge(&[&a, &b], dir.path(), &mut now, 4);
    let x = T0 - 3_600_000;
    for (p, v) in [(&a, 90), (&b, 95)] {
        let id: i64 = p.db.query_row("SELECT id FROM import WHERE status = 'review'", [], |r| r.get(0)).unwrap();
        let rows =
            vec![ObsRow { metric: GLU.into(), start_ms: x, end_ms: None, tz_min: 480, value: Value::Int(v), unit: None, detail: None }];
        now += 1_000;
        review::confirm(&p.db, id, &rows, Some(&format!("report {v}")), &[], now).unwrap();
    }
    converge(&[&a, &b], dir.path(), &mut now, 4);
    let glu = |c: &Db| -> Value {
        c.query_row(
            "SELECT o.value FROM observation o JOIN metric m ON m.id = o.metric_id WHERE m.code = ? AND o.start_ms = ?",
            rusqlite::params![GLU, x],
            |r| r.get(0),
        )
        .unwrap()
    };
    assert_eq!((glu(&a.db), glu(&b.db)), (Value::Int(95), Value::Int(95)), "R1: the larger value wins on both");
    assert_eq!(dump(&a.db), dump(&b.db));
}
