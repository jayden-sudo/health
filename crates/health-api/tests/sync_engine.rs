//! The LAN sync session (docs/lan-sync.md §6, §10) between profiles over an in-memory channel: frames go from one
//! session's `poll` to the other's `on_frame`, in random interleavings, sometimes cut off midway.

use health_api::sync::{Limits, Msg, Question, Session, Status};
use health_core::db::Db;
use health_core::keys::MemoryStore;
use health_core::profile::Profile;
use health_core::syncstate::SyncState;
use health_core::write::{Begin, ObsRow, SourceSpec, begin, finish, write};
use health_core::{Value, erase, sync};

const T0: i64 = 1_795_000_000_000;
const RHR: &str = "loinc:40443-4";

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

fn devices(n: usize, joined: bool) -> (tempfile::TempDir, Vec<Profile>) {
    let d = tempfile::tempdir().unwrap();
    let keys = MemoryStore::default();
    let ps: Vec<Profile> = (0..n).map(|i| Profile::init(d.path(), &format!("d{i}"), &keys).unwrap()).collect();
    if joined {
        for p in &ps {
            p.sync
                .update(|s| {
                    s.joined = true;
                    Ok(())
                })
                .unwrap();
        }
    }
    (d, ps)
}

fn small() -> Limits {
    Limits { batch_rows: 100, ..Default::default() }
}

fn import(c: &Db, t0: i64, n: i64, now: i64) -> (i64, String) {
    let src = SourceSpec::new("whoop", "u1");
    let (id, uid) = begin(c, Begin { source: &src, parser: "t", blob: None, uid: None, candidates: None }, now).unwrap();
    let rows: Vec<ObsRow> = (0..n)
        .map(|i| ObsRow {
            metric: RHR.into(),
            start_ms: t0 + i * 60_000,
            end_ms: None,
            tz_min: 480,
            value: Value::Int(50 + i % 30),
            unit: None,
            detail: None,
        })
        .collect();
    write(c, id, &rows).unwrap();
    finish(c, id, now).unwrap();
    (id, uid)
}

fn digest(p: &Profile) -> String {
    sync::of_profile(p).unwrap().digest()
}

/// Moves frames both ways until neither side has anything to send. `cut`: stop after this many frames (an
/// interrupted channel). Returns how many frames moved.
fn pump(a: (&Profile, &mut Session), b: (&Profile, &mut Session), now: i64, r: &mut Rng, cut: Option<u64>) -> u64 {
    let (pa, sa) = a;
    let (pb, sb) = b;
    let mut moved = 0;
    loop {
        let mut idle = true;
        // both sides each pass, in a random order and bursts, so messages interleave differently each run
        let first = r.below(2);
        for side in [first, 1 - first] {
            let burst = 1 + r.below(4);
            for _ in 0..burst {
                let (from, fp, to, tp) = if side == 0 { (&mut *sa, pa, &mut *sb, pb) } else { (&mut *sb, pb, &mut *sa, pa) };
                let Some(f) = from.poll(fp, now).unwrap() else { break };
                idle = false;
                moved += 1;
                let _ = to.on_frame(tp, now, &f);
                if cut.is_some_and(|c| moved >= c) {
                    return moved;
                }
            }
        }
        if idle {
            return moved;
        }
    }
}

fn session(p: &Profile, now: i64, limits: Limits) -> Session {
    Session::new(p, now, "test", None, limits).unwrap()
}

/// One full session between a and b.
fn sync_pair(a: &Profile, b: &Profile, now: i64, r: &mut Rng) -> (Status, Status) {
    let (mut sa, mut sb) = (session(a, now, small()), session(b, now, small()));
    pump((a, &mut sa), (b, &mut sb), now, r, None);
    assert!(sa.is_finished() && sb.is_finished(), "a session didn't end: {:?} / {:?}", sa.status(), sb.status());
    (sa.status().clone(), sb.status().clone())
}

#[test]
fn equal_devices_are_up_to_date_from_their_hellos() {
    let (_d, ps) = devices(2, true);
    let mut r = Rng(1);
    assert_eq!(sync_pair(&ps[0], &ps[1], T0, &mut r), (Status::UpToDate, Status::UpToDate));
    let st = ps[0].sync.load().unwrap();
    assert!(st.peers.values().all(|p| p.up_to_date && p.last_synced == Some(T0)));
}

#[test]
fn two_devices_exchange_what_each_lacks_in_one_session() {
    let (_d, ps) = devices(2, true);
    let mut r = Rng(2);
    for i in 0..5 {
        import(&ps[0].db, T0 - 86_400_000 + i * 3_600_000, 30, T0);
    }
    import(&ps[1].db, T0 - 2 * 86_400_000, 250, T0); // split: over the 100-row batches
    let (x, y) = sync_pair(&ps[0], &ps[1], T0 + 1000, &mut r);
    assert_eq!((x, y), (Status::UpToDate, Status::UpToDate));
    assert_eq!(digest(&ps[0]), digest(&ps[1]));
    // and the next session has nothing to do
    assert_eq!(sync_pair(&ps[0], &ps[1], T0 + 2000, &mut r), (Status::UpToDate, Status::UpToDate));
}

#[test]
fn a_device_asks_before_its_first_sync_and_a_no_ends_both_sides() {
    let (_d, ps) = devices(2, true);
    ps[1].sync.save(&SyncState::default()).unwrap(); // not joined
    import(&ps[0].db, T0 - 86_400_000, 10, T0);
    let mut r = Rng(3);
    let (mut sa, mut sb) = (session(&ps[0], T0, small()), session(&ps[1], T0, small()));
    assert_eq!(sb.status(), &Status::Asking(Question::Join { imports: 0 }));
    pump((&ps[0], &mut sa), (&ps[1], &mut sb), T0, &mut r, None);
    assert!(!sa.is_finished(), "a waits while b asks");
    sb.answer(&ps[1], T0, false).unwrap();
    pump((&ps[0], &mut sa), (&ps[1], &mut sb), T0, &mut r, None);
    assert_eq!(sa.status(), &Status::Ended { code: "declined".into() });
    assert!(!ps[1].sync.load().unwrap().joined);
    // asked again, allowed: synced
    let (mut sa, mut sb) = (session(&ps[0], T0, small()), session(&ps[1], T0, small()));
    pump((&ps[0], &mut sa), (&ps[1], &mut sb), T0, &mut r, None);
    sb.answer(&ps[1], T0, true).unwrap();
    pump((&ps[0], &mut sa), (&ps[1], &mut sb), T0, &mut r, None);
    assert_eq!((sa.status(), sb.status()), (&Status::UpToDate, &Status::UpToDate));
    assert!(ps[1].sync.load().unwrap().joined);
}

#[test]
fn a_large_erase_asks_and_not_now_pauses_sync() {
    let (_d, ps) = devices(2, true);
    let mut r = Rng(4);
    let ids: Vec<i64> = (0..25).map(|i| import(&ps[0].db, T0 - 86_400_000 + i * 3_600_000, 2, T0).0).collect();
    import(&ps[0].db, T0 - 5 * 86_400_000, 90, T0); // enough rows that one small erase is under 10%
    sync_pair(&ps[0], &ps[1], T0 + 1, &mut r);
    for id in &ids[..22] {
        erase::erase(&ps[0].db, *id, T0 + 2).unwrap();
    }
    let (mut sa, mut sb) = (session(&ps[0], T0 + 3, small()), session(&ps[1], T0 + 3, small()));
    pump((&ps[0], &mut sa), (&ps[1], &mut sb), T0 + 3, &mut r, None);
    assert_eq!(sb.status(), &Status::Asking(Question::Erase { imports: 22, rows: 44 }));
    sb.answer(&ps[1], T0 + 3, false).unwrap();
    pump((&ps[0], &mut sa), (&ps[1], &mut sb), T0 + 3, &mut r, None);
    assert_eq!(sa.status(), &Status::Ended { code: "paused".into() });
    assert!(ps[1].sync.load().unwrap().paused);
    assert_eq!(session(&ps[1], T0 + 4, small()).status(), &Status::Ended { code: "paused".into() });
    // continued: the erase goes through
    ps[1]
        .sync
        .update(|s| {
            s.paused = false;
            Ok(())
        })
        .unwrap();
    let (mut sa, mut sb) = (session(&ps[0], T0 + 5, small()), session(&ps[1], T0 + 5, small()));
    pump((&ps[0], &mut sa), (&ps[1], &mut sb), T0 + 5, &mut r, None);
    sb.answer(&ps[1], T0 + 5, true).unwrap();
    pump((&ps[0], &mut sa), (&ps[1], &mut sb), T0 + 5, &mut r, None);
    assert_eq!(digest(&ps[0]), digest(&ps[1]));
    // erasing imports that hold nothing (manual edits leave such tombstones) never asks: small erases neither
    erase::erase(&ps[0].db, ids[22], T0 + 6).unwrap();
    assert_eq!(sync_pair(&ps[0], &ps[1], T0 + 7, &mut r).1, Status::UpToDate);
}

#[test]
fn an_interrupted_split_import_resumes_where_it_stopped() {
    let (_d, ps) = devices(2, true);
    let mut r = Rng(5);
    import(&ps[0].db, T0 - 3 * 86_400_000, 450, T0); // 5 parts of 100
    let (mut sa, mut sb) = (session(&ps[0], T0, small()), session(&ps[1], T0, small()));
    // cut after a few parts arrived
    let mut tries = 0;
    while ps[1].sync.load().unwrap().resume.is_empty() {
        pump((&ps[0], &mut sa), (&ps[1], &mut sb), T0, &mut r, Some(1));
        tries += 1;
        assert!(tries < 10_000, "no part arrived: {:?} / {:?}", sa.status(), sb.status());
    }
    drop((sa, sb));
    let cursor = ps[1].sync.load().unwrap().resume.values().next().cloned().unwrap();
    let (mut sa, mut sb) = (session(&ps[0], T0 + 1, small()), session(&ps[1], T0 + 1, small()));
    pump((&ps[0], &mut sa), (&ps[1], &mut sb), T0 + 1, &mut r, None);
    assert_eq!((sa.status(), sb.status()), (&Status::UpToDate, &Status::UpToDate));
    assert!(sb.report().packs < 5, "resumed after {cursor:?}: only the rest was sent ({} packs)", sb.report().packs);
    assert!(ps[1].sync.load().unwrap().resume.is_empty());
}

#[test]
fn what_a_merge_cant_equalise_is_cached_and_the_next_session_ends_at_once() {
    // the same measure code made with different units on two devices: its rows are skipped (merge-fixes R4), so
    // each device's import stays different on the other side for good
    let (_d, ps) = devices(2, true);
    let mut r = Rng(6);
    let code = "x:ocr/test";
    for (i, (p, unit)) in [(&ps[0], "mg"), (&ps[1], "g")].into_iter().enumerate() {
        p.db.execute("INSERT INTO metric (code, name, unit, agg) VALUES (?, 'test', ?, 'instant')", [code, unit]).unwrap();
        let src = SourceSpec::new("ocr", &format!("photo{i}"));
        let (id, _) = begin(&p.db, Begin { source: &src, parser: "t", blob: None, uid: None, candidates: None }, T0).unwrap();
        let rows = [ObsRow {
            metric: code.into(),
            start_ms: T0 - 5000,
            end_ms: None,
            tz_min: 480,
            value: Value::Int(5),
            unit: None,
            detail: None,
        }];
        write(&p.db, id, &rows).unwrap();
        finish(&p.db, id, T0).unwrap();
    }
    // the first round brings each import over (without the skipped rows): progress, nothing cached yet
    assert_eq!(sync_pair(&ps[0], &ps[1], T0 + 2, &mut r), (Status::Partly { skipped: 0 }, Status::Partly { skipped: 0 }));
    // the second changes nothing: cached. Both imports differ both ways (each side lacks the other's rows)
    let (x, y) = sync_pair(&ps[0], &ps[1], T0 + 2, &mut r);
    assert_eq!((x, y), (Status::Partly { skipped: 2 }, Status::Partly { skipped: 2 }));
    // no pack the second time: nothing is wanted again while both sides stay as they are
    let (mut sa, mut sb) = (session(&ps[0], T0 + 3, small()), session(&ps[1], T0 + 3, small()));
    pump((&ps[0], &mut sa), (&ps[1], &mut sb), T0 + 3, &mut r, None);
    assert_eq!((sa.report().packs, sb.report().packs), (0, 0));
    assert!(sa.is_finished() && sb.is_finished());
    assert_eq!(sa.status(), &Status::Partly { skipped: 2 });
    // fixed on one side (a unit isn't in the inventory): retried after a day, then equal
    ps[1].db.execute("UPDATE metric SET unit = 'mg' WHERE code = ?", [code]).unwrap();
    assert_eq!(sync_pair(&ps[0], &ps[1], T0 + 4, &mut r).0, Status::Partly { skipped: 2 });
    let later = T0 + health_api::sync::SKIP_RETRY_MS + 10;
    assert_eq!(sync_pair(&ps[0], &ps[1], later, &mut r), (Status::UpToDate, Status::UpToDate));
}

#[test]
fn files_travel_with_their_imports() {
    let (_d, ps) = devices(2, true);
    let mut r = Rng(7);
    let f = ps[0].tmp().join("photo");
    std::fs::write(&f, b"synthetic photo bytes").unwrap();
    let sha = ps[0].blobs.put_file(&f).unwrap();
    let src = SourceSpec::new("ocr", "photo");
    let (id, _) = begin(&ps[0].db, Begin { source: &src, parser: "ocr", blob: Some(&sha), uid: None, candidates: None }, T0).unwrap();
    let rows =
        [ObsRow { metric: RHR.into(), start_ms: T0 - 5000, end_ms: None, tz_min: 480, value: Value::Int(55), unit: None, detail: None }];
    write(&ps[0].db, id, &rows).unwrap();
    finish(&ps[0].db, id, T0).unwrap();
    assert_eq!(sync_pair(&ps[0], &ps[1], T0 + 1, &mut r), (Status::UpToDate, Status::UpToDate));
    assert!(ps[1].blobs.has(&sha));
}

#[test]
fn profile_and_whoop_stop_settings_reach_the_other_device() {
    let (_d, ps) = devices(2, true);
    let mut r = Rng(8);
    ps[1]
        .vault
        .update(|s| {
            s.whoop = Some(health_core::vault::Whoop { connected_at: Some(T0 - 10_000), ..Default::default() });
            Ok(())
        })
        .unwrap();
    let dev = ps[0].vault.device_id().unwrap();
    ps[0]
        .vault
        .update(|s| {
            s.profile = Some(health_core::vault::Personal { birth: Some("1985-04".into()), gender: Some("Male".into()), ethnicity: None });
            s.profile_stamp = Some(health_core::vault::Stamp { set_at: T0 << 16, set_by: dev.clone() });
            sync::stop_whoop(&ps[0].db, s, &dev, T0)
        })
        .unwrap();
    let (mut sa, mut sb) = (session(&ps[0], T0 + 1, small()), session(&ps[1], T0 + 1, small()));
    pump((&ps[0], &mut sa), (&ps[1], &mut sb), T0 + 1, &mut r, None);
    assert_eq!((sa.status(), sb.status()), (&Status::UpToDate, &Status::UpToDate));
    assert!(sb.report().whoop_disconnected);
    let v = ps[1].vault.load().unwrap();
    assert!(v.whoop.is_none());
    assert_eq!(v.profile.unwrap().birth.as_deref(), Some("1985-04"));
}

#[test]
fn a_peer_over_the_limits_or_out_of_turn_ends_the_session() {
    let (_d, ps) = devices(1, true);
    let p = &ps[0];
    let tight = Limits { control: 1000, ..Default::default() };
    let f = |kind: u8, last: u8, body: &[u8]| [vec![kind, last], body.to_vec()].concat();
    let code = |s: &Session| match s.status() {
        Status::Ended { code } => code.clone(),
        x => format!("{x:?}"),
    };
    let mut s = session(p, T0, tight.clone());
    assert!(s.on_frame(p, T0, &f(b'c', 0, &[b' '; 900])).is_ok());
    assert_eq!(s.on_frame(p, T0, &f(b'c', 1, &[b' '; 200])).unwrap_err().code, "sync_large");
    assert_eq!(code(&s), "sync_large");
    let mut s = session(p, T0, Limits::default());
    assert_eq!(s.on_frame(p, T0, &f(b'x', 1, b"")).unwrap_err().code, "sync_frame");
    let mut s = session(p, T0, Limits::default());
    assert_eq!(s.on_frame(p, T0, &vec![b'c'; health_api::sync::MAX_FRAME + 1]).unwrap_err().code, "sync_frame");
    // a pack nobody asked for, a want before the inventories
    for m in [
        Msg::Pack { bytes: 10, password: "x".into(), imports: 0, part: None },
        Msg::Served,
        Msg::Want { imports: vec![], after: Default::default(), corrections: false, settings: false, aliases: false, blobs: vec![] },
    ] {
        let mut s = session(p, T0, Limits::default());
        let body = serde_json::to_vec(&m).unwrap();
        assert_eq!(s.on_frame(p, T0, &f(b'c', 1, &body)).unwrap_err().code, "sync_order", "{m:?}");
    }
    // its own device id
    let mut s = session(p, T0, Limits::default());
    let me = p.vault.device_id().unwrap();
    let hello = Msg::Hello { proto: 1, device: me, name: String::new(), digest: String::new(), lacks: 0, max_pack: 1 << 30, files: true };
    assert_eq!(s.on_frame(p, T0, &f(b'c', 1, &serde_json::to_vec(&hello).unwrap())).unwrap_err().code, "sync_peer");
    // and a bye goes out
    let mut out = Vec::new();
    while let Some(fr) = s.poll(p, T0).unwrap() {
        out.push(fr);
    }
    let last: Msg = serde_json::from_slice(&out.last().unwrap()[2..]).unwrap();
    assert_eq!(last, Msg::Bye { code: "sync_peer".into() });
}

#[test]
fn ops_stamp_the_synced_settings_with_this_device() {
    use health_api::{Caller, Ctx, call};
    use serde_json::json;
    let (_d, ps) = devices(1, true);
    let p = &ps[0];
    let me = p.vault.device_id().unwrap();
    import(&p.db, T0 - 86_400_000, 3, T0);
    // a source edit carries set_by
    let sid: i64 = p.db.query_row("SELECT id FROM source WHERE connector = 'whoop'", [], |r| r.get(0)).unwrap();
    call(&mut Ctx::new(p, T0, Caller::Owner), "source.update", json!({ "id": sid, "priority": 5 })).unwrap();
    let meta: String = p.db.query_row("SELECT meta FROM source WHERE id = ?", [sid], |r| r.get(0)).unwrap();
    assert!(meta.contains(&me), "{meta}");
    // profile.set stamps the profile
    call(&mut Ctx::new(p, T0, Caller::Owner), "profile.set", json!({ "birth": "1980-01" })).unwrap();
    let v = p.vault.load().unwrap();
    assert_eq!(v.profile_stamp.as_ref().map(|s| s.set_by.as_str()), Some(me.as_str()));
    let set_at = v.profile_stamp.unwrap().set_at;
    assert!(set_at >= T0 << 16);
    // deleting WHOOP's data stops WHOOP, but only once committed: a dry run leaves the connection
    p.vault
        .update(|s| {
            s.whoop = Some(health_core::vault::Whoop { connected_at: Some(T0 - 1), ..Default::default() });
            Ok(())
        })
        .unwrap();
    let mut dry = Ctx::new(p, T0 + 1, Caller::Owner);
    dry.dry_run = true;
    call(&mut dry, "import.erase_source", json!({ "connector": "whoop" })).unwrap();
    let v = p.vault.load().unwrap();
    assert!(v.whoop.is_some() && v.whoop_stopped.is_none());
    call(&mut Ctx::new(p, T0 + 1, Caller::Owner), "import.erase_source", json!({ "connector": "whoop" })).unwrap();
    let v = p.vault.load().unwrap();
    assert!(v.whoop.is_none());
    let stop = v.whoop_stopped.unwrap();
    assert_eq!((stop.at, stop.stamp.set_by.as_str()), (T0 + 1, me.as_str()));
    assert!(stop.stamp.set_at > set_at, "stamps only grow");
}

/// Runs frame by frame until `until` holds (or the sessions go idle).
fn pump_until(a: (&Profile, &mut Session), b: (&Profile, &mut Session), now: i64, r: &mut Rng, until: &dyn Fn(&Session, &Session) -> bool) {
    let (pa, sa) = a;
    let (pb, sb) = b;
    for _ in 0..100_000 {
        if until(sa, sb) {
            return;
        }
        if pump((pa, &mut *sa), (pb, &mut *sb), now, r, Some(1)) == 0 {
            return;
        }
    }
    panic!("never reached");
}

fn wanting(s: &Session) -> bool {
    matches!(s.status(), Status::Syncing { wanted, .. } if *wanted > 0)
}

#[test]
fn a_peer_that_erases_during_the_session_ends_it_before_the_erase_is_merged() {
    let (_d, ps) = devices(2, true);
    let mut r = Rng(9);
    let (id, uid) = import(&ps[0].db, T0 - 86_400_000, 3, T0);
    import(&ps[0].db, T0 - 5 * 86_400_000, 90, T0); // enough rows that erasing 3 doesn't ask (§10)
    sync_pair(&ps[0], &ps[1], T0, &mut r);
    // b holds the import's rows; a changes it (a corrected row value), so b wants it again
    ps[0].db.execute("UPDATE observation SET value = 77 WHERE import_id = ?", [id]).unwrap();
    let (mut sa, mut sb) = (session(&ps[0], T0 + 1, small()), session(&ps[1], T0 + 1, small()));
    pump_until((&ps[0], &mut sa), (&ps[1], &mut sb), T0 + 1, &mut r, &|_, b| wanting(b));
    // a erases it before serving: a tombstone b's inventory never showed, deleting rows here
    erase::erase(&ps[0].db, id, T0 + 1).unwrap();
    pump((&ps[0], &mut sa), (&ps[1], &mut sb), T0 + 1, &mut r, None);
    assert_eq!(sb.status(), &Status::Ended { code: "sync_changed".into() });
    assert_eq!(sync::of_profile(&ps[1]).unwrap().imports[&uid].status, "done", "nothing of it merged");
    // the next session sees the tombstone in the inventory and goes through
    assert_eq!(sync_pair(&ps[0], &ps[1], T0 + 2, &mut r), (Status::UpToDate, Status::UpToDate));
}

#[test]
fn a_tombstone_that_deletes_nothing_here_is_taken_mid_session() {
    let (_d, ps) = devices(2, true);
    let mut r = Rng(11);
    let (id, uid) = import(&ps[0].db, T0 - 86_400_000, 3, T0);
    let (mut sa, mut sb) = (session(&ps[0], T0, small()), session(&ps[1], T0, small()));
    pump_until((&ps[0], &mut sa), (&ps[1], &mut sb), T0, &mut r, &|_, b| wanting(b));
    erase::erase(&ps[0].db, id, T0).unwrap();
    pump((&ps[0], &mut sa), (&ps[1], &mut sb), T0, &mut r, None);
    assert!(!matches!(sb.status(), Status::Ended { .. }), "{:?}", sb.status());
    assert_eq!(sync::of_profile(&ps[1]).unwrap().imports[&uid].status, "erased");
}

#[test]
fn a_pack_over_the_limit_or_short_of_its_size_ends_the_session() {
    for (bytes, body, code) in [(3u64 << 30, 0usize, "sync_large"), (10, 5, "sync_frame")] {
        let (_d, ps) = devices(2, true);
        let mut r = Rng(10);
        import(&ps[0].db, T0 - 86_400_000, 3, T0);
        let (mut sa, mut sb) = (session(&ps[0], T0, small()), session(&ps[1], T0, small()));
        pump_until((&ps[0], &mut sa), (&ps[1], &mut sb), T0, &mut r, &|_, b| wanting(b));
        let m = serde_json::to_vec(&Msg::Pack { bytes, password: "x".into(), imports: 1, part: None }).unwrap();
        let _ = sb.on_frame(&ps[1], T0, &[vec![b'c', 1], m].concat());
        if body > 0 {
            let _ = sb.on_frame(&ps[1], T0, &[vec![b'p', 1], vec![0u8; body]].concat());
        }
        assert_eq!(sb.status(), &Status::Ended { code: code.into() });
        assert_eq!(std::fs::read_dir(ps[1].tmp()).unwrap().count(), 0, "no partial pack left");
    }
}

#[test]
fn erasing_here_makes_a_cached_import_wanted_again() {
    // a holds X and a newer Y over the same row; b holds X with the row. a pulls X: nothing changes (Y owns the
    // row), so it's cached. Before b gets Y, a erases Y: the row must come back to a from b's X.
    let t = T0 - 86_400_000;
    for seed in 0..40 {
        let (_d, ps) = devices(2, true);
        let mut r = Rng(100 + seed);
        let (a, b) = (&ps[0], &ps[1]);
        let (_, x) = import(&a.db, t, 1, T0);
        sync_pair(a, b, T0 + 1, &mut r);
        let (y, _) = import(&a.db, t, 1, T0 + 2);
        a.db.execute("UPDATE observation SET value = 99 WHERE import_id = ?", [y]).unwrap();
        // b's X differs from a's now (a's X owns nothing): one session, cut once a has cached X while b lacks Y
        let (mut sa, mut sb) = (session(a, T0 + 3, small()), session(b, T0 + 3, small()));
        pump_until((a, &mut sa), (b, &mut sb), T0 + 3, &mut r, &|_, _| {
            a.sync.load().unwrap().skip.iter().any(|k| k.what == format!("i {x}"))
        });
        let b_has_y: i64 = b.db.query_row("SELECT count(*) FROM observation WHERE value = 99", [], |r| r.get(0)).unwrap();
        if b_has_y > 0 || !a.sync.load().unwrap().skip.iter().any(|k| k.what == format!("i {x}")) {
            continue; // this interleaving delivered Y first: try another
        }
        drop((sa, sb));
        erase::erase(&a.db, y, T0 + 4).unwrap();
        for i in 0..3 {
            sync_pair(a, b, T0 + 5 + i, &mut r);
        }
        let v: Option<i64> = a.db.query_row("SELECT value FROM v_observation", [], |r| r.get(0)).ok();
        assert_eq!(v, Some(50), "seed {seed}: the row came back from b's X");
        assert_eq!(digest(a), digest(b));
        return;
    }
    panic!("no interleaving cached X before b got Y");
}

fn put_file(p: &Profile, bytes: usize, seed: u8) -> String {
    let f = p.tmp().join(format!("f{seed}"));
    let data: Vec<u8> = (0..bytes).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed)).collect();
    std::fs::write(&f, data).unwrap();
    p.blobs.put_file(&f).unwrap()
}

fn import_with_file(p: &Profile, sha: &str, t: i64) {
    let src = SourceSpec::new("ocr", &format!("photo-{}", &sha[..8]));
    let (id, _) = begin(&p.db, Begin { source: &src, parser: "ocr", blob: Some(sha), uid: None, candidates: None }, T0).unwrap();
    let rows = [ObsRow { metric: RHR.into(), start_ms: t, end_ms: None, tz_min: 480, value: Value::Int(55), unit: None, detail: None }];
    write(&p.db, id, &rows).unwrap();
    finish(&p.db, id, T0).unwrap();
}

#[test]
fn a_pack_too_large_for_the_peer_is_split_or_left_out_never_ending_the_session() {
    let (_d, ps) = devices(2, true);
    let mut r = Rng(12);
    // the receiver takes packs of 1 MiB; one file is larger, the others fit
    let big = put_file(&ps[0], 1_500_000, 1);
    import_with_file(&ps[0], &big, T0 - 5000);
    let small: Vec<String> = (0..3).map(|i| put_file(&ps[0], 300_000, 10 + i)).collect();
    for (i, f) in small.iter().enumerate() {
        import_with_file(&ps[0], f, T0 - 10_000 - i as i64);
    }
    let tight = Limits { pack: 1 << 20, ..small_limits() };
    let (mut sa, mut sb) = (session(&ps[0], T0, small_limits()), session(&ps[1], T0, tight.clone()));
    pump((&ps[0], &mut sa), (&ps[1], &mut sb), T0, &mut r, None);
    assert!(sa.is_finished() && sb.is_finished());
    assert!(!matches!(sb.status(), Status::Ended { .. }), "{:?}", sb.status());
    assert!(sa.report().oversize > 0);
    // every import arrived; the small files too, the large one not
    assert_eq!(sync::of_profile(&ps[1]).unwrap().imports.len(), 4);
    assert!(small.iter().all(|f| ps[1].blobs.has(f)) && !ps[1].blobs.has(&big));
    // the next session asks for the missing file once (it left with its import): too large, so cached
    let (mut sa, mut sb) = (session(&ps[0], T0 + 1, small_limits()), session(&ps[1], T0 + 1, tight.clone()));
    pump((&ps[0], &mut sa), (&ps[1], &mut sb), T0 + 1, &mut r, None);
    assert_eq!((sa.report().oversize, sb.status()), (1, &Status::Partly { skipped: 1 }));
    // and the one after doesn't try again
    let (mut sa, mut sb) = (session(&ps[0], T0 + 2, small_limits()), session(&ps[1], T0 + 2, tight));
    pump((&ps[0], &mut sa), (&ps[1], &mut sb), T0 + 2, &mut r, None);
    assert_eq!((sa.report().oversize, sb.status()), (0, &Status::Partly { skipped: 1 }));
}

fn small_limits() -> Limits {
    small()
}

#[test]
fn a_peer_without_files_syncs_the_data_and_is_up_to_date() {
    let (_d, ps) = devices(2, true);
    let mut r = Rng(13);
    let f = put_file(&ps[0], 1000, 3);
    import_with_file(&ps[0], &f, T0 - 5000);
    let web = Limits { files: false, ..small() };
    let (mut sa, mut sb) = (session(&ps[0], T0, small()), session(&ps[1], T0, web.clone()));
    pump((&ps[0], &mut sa), (&ps[1], &mut sb), T0, &mut r, None);
    assert_eq!((sa.status(), sb.status()), (&Status::UpToDate, &Status::UpToDate));
    assert!(!ps[1].blobs.has(&f), "no file sent to a side that keeps none");
    let (mut sa, mut sb) = (session(&ps[0], T0 + 1, small()), session(&ps[1], T0 + 1, web));
    pump((&ps[0], &mut sa), (&ps[1], &mut sb), T0 + 1, &mut r, None);
    assert_eq!((sa.status(), sb.status()), (&Status::UpToDate, &Status::UpToDate), "the hello shortcut too");
    assert_eq!(sa.report().packs + sb.report().packs, 0);
}

#[test]
fn a_damaged_sync_file_starts_over_and_asks_to_join_again() {
    let (_d, ps) = devices(1, true);
    std::fs::write(ps[0].dir.join("sync"), vec![0u8; 100]).unwrap();
    let s = session(&ps[0], T0, small());
    assert_eq!(s.status(), &Status::Asking(Question::Join { imports: 0 }));
    assert!(!ps[0].sync.load().unwrap().joined);
}

#[test]
fn a_stale_resume_cursor_goes_once_its_import_is_erased() {
    let (_d, ps) = devices(2, true);
    let mut r = Rng(14);
    let (id, _) = import(&ps[0].db, T0 - 3 * 86_400_000, 450, T0);
    let (mut sa, mut sb) = (session(&ps[0], T0, small()), session(&ps[1], T0, small()));
    let mut tries = 0;
    while ps[1].sync.load().unwrap().resume.is_empty() {
        pump((&ps[0], &mut sa), (&ps[1], &mut sb), T0, &mut r, Some(1));
        tries += 1;
        assert!(tries < 10_000);
    }
    drop((sa, sb));
    erase::erase(&ps[0].db, id, T0 + 1).unwrap();
    let (mut sa, mut sb) = (session(&ps[0], T0 + 2, small()), session(&ps[1], T0 + 2, small()));
    pump((&ps[0], &mut sa), (&ps[1], &mut sb), T0 + 2, &mut r, None);
    if let Status::Asking(_) = sb.status() {
        sb.answer(&ps[1], T0 + 2, true).unwrap();
        pump((&ps[0], &mut sa), (&ps[1], &mut sb), T0 + 2, &mut r, None);
    }
    assert_eq!(digest(&ps[0]), digest(&ps[1]));
    assert!(ps[1].sync.load().unwrap().resume.is_empty(), "{:?}", ps[1].sync.load().unwrap().resume);
}

#[test]
fn a_part_of_an_import_nobody_asked_for_ends_the_session() {
    let (_d, ps) = devices(2, true);
    let mut r = Rng(15);
    import(&ps[0].db, T0 - 86_400_000, 3, T0);
    let (mut sa, mut sb) = (session(&ps[0], T0, small()), session(&ps[1], T0, small()));
    pump_until((&ps[0], &mut sa), (&ps[1], &mut sb), T0, &mut r, &|_, b| wanting(b));
    let part = health_api::sync::Part { uid: "junk".into(), next: None };
    let m = serde_json::to_vec(&Msg::Pack { bytes: 10, password: "x".into(), imports: 1, part: Some(part) }).unwrap();
    let _ = sb.on_frame(&ps[1], T0, &[vec![b'c', 1], m].concat());
    assert_eq!(sb.status(), &Status::Ended { code: "sync_pack".into() });
    assert!(ps[1].sync.load().unwrap().resume.is_empty());
}

#[test]
fn a_peer_that_starts_over_ends_the_session_quietly_and_a_new_one_syncs() {
    let (_d, ps) = devices(2, true);
    let mut r = Rng(16);
    import(&ps[0].db, T0 - 86_400_000, 3, T0);
    let (mut sa, mut sb) = (session(&ps[0], T0, small()), session(&ps[1], T0, small()));
    pump_until((&ps[0], &mut sa), (&ps[1], &mut sb), T0, &mut r, &|_, b| wanting(b));
    // a's side drops its session (a stall there) and starts a new one: its hello reaches b mid-session
    drop(sa);
    let mut sa = session(&ps[0], T0, small());
    let hello = sa.poll(&ps[0], T0).unwrap().unwrap();
    sb.on_frame(&ps[1], T0, &hello).unwrap();
    assert!(sb.restarted() && sb.is_finished(), "{:?}", sb.status());
    assert_eq!(sb.poll(&ps[1], T0).unwrap(), None, "no bye: the peer's new session goes on");
    let mut sb = session(&ps[1], T0, small());
    sb.on_frame(&ps[1], T0, &hello).unwrap();
    pump((&ps[0], &mut sa), (&ps[1], &mut sb), T0, &mut r, None);
    assert_eq!((sa.status(), sb.status()), (&Status::UpToDate, &Status::UpToDate));
}
