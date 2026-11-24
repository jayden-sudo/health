//! Sessions under random conditions (docs/lan-sync.md §6, S2): three devices make the random changes of health-core's
//! sync test, and sync in random pairs through real sessions: random frame interleavings, channels cut after a
//! random frame, local changes in the middle of a session. Then sessions until every digest agrees, and the tables
//! must be identical, daily equal to a full rebuild.

#[path = "../../health-core/tests/common/steps.rs"]
mod steps;

use health_api::sync::{Limits, Session, Status};
use health_core::keys::MemoryStore;
use health_core::profile::Profile;
use health_core::{daily, sync};
use steps::*;

/// Device 2 keeps no files, as the web (lan-sync §6.4).
fn limits(d: usize) -> Limits {
    Limits { batch_rows: 3, files: d != 2, ..Default::default() }
}

/// Changes the shared steps don't make: an import large enough to be split (batch_rows is 3), and an import with a
/// real file.
fn extra(p: &Profile, d: usize, r: &mut Rng, now: i64) {
    use health_core::write::{Begin, ObsRow, SourceSpec, begin, finish, write};
    let rows: Vec<ObsRow> = (0..4 + r.below(6) as i64)
        .map(|i| ObsRow {
            metric: RHR.into(),
            start_ms: T0 - 2 * 86_400_000 + r.below(3) as i64 * 3_600_000 + i * 60_000,
            end_ms: None,
            tz_min: 480,
            value: health_core::Value::Int(40 + r.below(5) as i64),
            unit: None,
            detail: None,
        })
        .collect();
    let blob = if r.below(2) == 0 && d != 2 {
        let f = p.tmp().join(format!("x{now}"));
        std::fs::write(&f, format!("file {now} {d}")).unwrap();
        Some(p.blobs.put_file(&f).unwrap())
    } else {
        None
    };
    let src = SourceSpec::new("apple_health", "watch");
    if let Ok((id, _)) = begin(&p.db, Begin { source: &src, parser: "t", blob: blob.as_deref(), uid: None, candidates: None }, now) {
        write(&p.db, id, &rows).unwrap();
        finish(&p.db, id, now).unwrap();
    }
}

/// Runs a session between x and y. `cut`: the channel drops after that many frames; `mid`: a change on x after
/// that many frames. Questions are answered yes. Returns whether it ended without a cut.
fn session(devs: &[Profile], x: usize, y: usize, now: i64, r: &mut Rng, cut: Option<u64>, mid: Option<u64>) -> bool {
    let (px, py) = (&devs[x], &devs[y]);
    let mut sx = Session::new(px, now, "x", None, limits(x)).unwrap();
    let mut sy = Session::new(py, now, "y", None, limits(y)).unwrap();
    let mut moved = 0u64;
    loop {
        let mut idle = true;
        let first = r.below(2);
        for side in [first, 1 - first] {
            for _ in 0..1 + r.below(3) {
                let (from, fp, to, tp) = if side == 0 { (&mut sx, px, &mut sy, py) } else { (&mut sy, py, &mut sx, px) };
                let Some(f) = from.poll(fp, now).unwrap() else { break };
                idle = false;
                moved += 1;
                let _ = to.on_frame(tp, now, &f);
                if mid == Some(moved) {
                    step(px, x, r, now);
                }
                if cut == Some(moved) {
                    return false;
                }
            }
        }
        for (s, p) in [(&mut sx, px), (&mut sy, py)] {
            if matches!(s.status(), Status::Asking(_)) {
                s.answer(p, now, true).unwrap();
                idle = false;
            }
        }
        if idle {
            assert!(sx.is_finished() && sy.is_finished(), "stuck: {:?} / {:?}", sx.status(), sy.status());
            for s in [&sx, &sy] {
                // a change here mid-session may erase what the peer asked for: that session ends (lan-sync §6.3)
                let changed = mid.is_some() && matches!(s.status(), Status::Ended { code } if code == "sync_changed");
                assert!(
                    changed || !matches!(s.status(), Status::Ended { .. }),
                    "ended: {:?} (x {x}, y {y}, now {now}, mid {mid:?})",
                    s.status()
                );
            }
            return true;
        }
    }
}

fn digest(p: &Profile) -> String {
    sync::of_profile(p).unwrap().digest()
}

fn run(seed: u64) {
    let dir = tempfile::tempdir().unwrap();
    let keys = MemoryStore::default();
    let devs: Vec<Profile> = (0..3).map(|i| Profile::init(dir.path(), &format!("d{i}"), &keys).unwrap()).collect();
    for p in &devs {
        p.sync
            .update(|s| {
                s.joined = true;
                Ok(())
            })
            .unwrap();
    }
    let mut r = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let mut now = T0;
    for _ in 0..40 {
        now += 1_000;
        let d = r.below(3) as usize;
        if r.below(5) == 0 {
            extra(&devs[d], d, &mut r, now);
        } else {
            step(&devs[d], d, &mut r, now);
        }
        if r.below(3) == 0 {
            let (x, y) = (r.below(3) as usize, r.below(3) as usize);
            if x != y {
                let cut = (r.below(3) == 0).then(|| 1 + r.below(40));
                let mid = (r.below(3) == 0).then(|| 1 + r.below(20));
                session(&devs, x, y, now, &mut r, cut, mid);
            }
        }
    }
    let mut rounds = 0;
    loop {
        let d: Vec<String> = devs.iter().map(digest).collect();
        if d.iter().all(|x| *x == d[0]) {
            break;
        }
        rounds += 1;
        assert!(rounds <= 8, "seed {seed}: no convergence after 8 rounds");
        for (x, y) in [(0, 1), (1, 2), (0, 2), (2, 1)] {
            now += 1_000;
            session(&devs, x, y, now, &mut r, None, None);
        }
    }
    // once more between every pair: up to date, nothing cached
    for (x, y) in [(0, 1), (1, 2), (0, 2)] {
        now += 1_000;
        session(&devs, x, y, now, &mut r, None, None);
    }
    // the devices that keep files hold every file any of them has
    for (i, p) in devs.iter().enumerate().take(2) {
        assert!(
            sync::of_profile(p).unwrap().lacks.iter().all(|b| !devs[0].blobs.has(b) && !devs[1].blobs.has(b)),
            "seed {seed}: device {i} lacks a file another holds"
        );
    }
    let first = dump(&devs[0].db);
    for (i, p) in devs.iter().enumerate() {
        assert_eq!(dump(&p.db), first, "seed {seed}: device {i} differs from device 0");
        let before = daily_text(&p.db);
        daily::rebuild_all(&p.db, now).unwrap();
        assert_eq!(daily_text(&p.db), before, "seed {seed}: device {i}'s daily differs from a full rebuild");
        assert!(p.sync.load().unwrap().skip.is_empty(), "seed {seed}: device {i} cached {:?}", p.sync.load().unwrap().skip);
        // nothing left in tmp/
        assert_eq!(
            std::fs::read_dir(p.tmp()).unwrap().filter(|e| e.as_ref().unwrap().file_name().to_string_lossy().starts_with("sync-")).count(),
            0,
            "seed {seed}: device {i} left temp files"
        );
        assert!(p.sync.load().unwrap().resume.is_empty(), "seed {seed}: device {i} kept a cursor");
    }
}

#[test]
fn three_devices_converge_through_sessions_under_random_cuts_and_interleavings() {
    // SEEDS=n for a deeper run
    let seeds: u64 = std::env::var("SEEDS").ok().and_then(|s| s.parse().ok()).unwrap_or(10);
    for seed in 1..=seeds {
        run(seed);
    }
}
