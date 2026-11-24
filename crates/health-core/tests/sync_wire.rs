//! The inventory on the wire (docs/lan-sync.md §6.1) and the vault's synced settings (§8).

use health_core::keys::MemoryStore;
use health_core::profile::Profile;
use health_core::sync::{self, Inventory, ProfileSetting, Settings};
use health_core::vault::{Stamp, State, Stopped, Whoop};
use health_core::write::{Begin, ObsRow, SourceSpec, begin, finish, write};
use health_core::{Value, query};

const T0: i64 = 1_795_000_000_000;

fn profile_with_data() -> (tempfile::TempDir, Profile) {
    let d = tempfile::tempdir().unwrap();
    let p = Profile::init(d.path(), "a", &MemoryStore::default()).unwrap();
    // a source name with a newline and spaces: text fields are read by length, not split on lines
    let src = SourceSpec::new("lab", "dev\ni zz done - -");
    let (id, _) = begin(&p.db, Begin { source: &src, parser: "t", blob: Some(&"e".repeat(64)), uid: None, candidates: None }, T0).unwrap();
    let rows = [ObsRow {
        metric: "loinc:2345-7".into(),
        start_ms: T0 - 1000,
        end_ms: None,
        tz_min: 480,
        value: Value::Real(5.5),
        unit: None,
        detail: None,
    }];
    write(&p.db, id, &rows).unwrap();
    finish(&p.db, id, T0).unwrap();
    let sid: i64 = p.db.query_row("SELECT id FROM source WHERE connector = 'lab'", [], |r| r.get(0)).unwrap();
    query::source_update(&p.db, sid, Some("My\nlab"), Some(7), T0, "dev-a").unwrap();
    (d, p)
}

#[test]
fn the_wire_text_reads_back_to_the_same_inventory() {
    let (_d, p) = profile_with_data();
    let inv = sync::of_profile(&p).unwrap();
    assert_eq!(inv.lacks.len(), 1, "the import's file isn't on disk");
    let back = Inventory::parse(&inv.wire()).unwrap();
    assert_eq!(back.digest(), inv.digest());
    assert_eq!(back.lacks, inv.lacks);
    assert_eq!((back.imports, back.corrections, back.settings, back.aliases), (inv.imports, inv.corrections, inv.settings, inv.aliases));
}

#[test]
fn malformed_or_unsorted_inventories_are_refused() {
    let (_d, p) = profile_with_data();
    let wire = sync::of_profile(&p).unwrap().wire();
    let lines: Vec<&str> = wire.split_inclusive('\n').collect();
    let refused = |s: &str| Inventory::parse(s).err().map(|e| e.code);
    // sections out of order
    let mut swapped = lines.clone();
    let (i, s) = (swapped.iter().position(|l| l.starts_with("i ")).unwrap(), swapped.iter().position(|l| l.starts_with("s ")).unwrap());
    swapped.swap(i, s);
    assert_eq!(refused(&swapped.concat()), Some("bad_inventory"));
    // a text length that runs past the end, an unknown tag, a missing final newline, a duplicate uid
    assert_eq!(refused("a 99:x 1:y 1:z\n"), Some("bad_inventory"));
    assert_eq!(refused("x 1\n"), Some("bad_inventory"));
    assert_eq!(refused("a 1:x 1:y 1:z"), Some("bad_inventory"));
    let fp = format!("1:{}", "0".repeat(64));
    assert_eq!(refused(&format!("i u1 done - {fp}\ni u1 erased - -\n")), Some("bad_inventory"));
    // a done import without a fingerprint, an erased one with one
    assert_eq!(refused("i u1 done - -\n"), Some("bad_inventory"));
    assert_eq!(refused(&format!("i u1 erased - {fp}\n")), Some("bad_inventory"));
    // too many lines
    let many: String = (0..=sync::MAX_LINES).map(|i| format!("b {:064x}\n", i)).collect();
    assert_eq!(refused(&many), Some("inventory_large"));
    assert!(Inventory::parse("").unwrap().imports.is_empty());
}

#[test]
fn a_running_import_is_never_wanted() {
    let d = tempfile::tempdir().unwrap();
    let keys = MemoryStore::default();
    let (a, b) = (Profile::init(d.path(), "a", &keys).unwrap(), Profile::init(d.path(), "b", &keys).unwrap());
    let src = SourceSpec::new("lab", "x");
    let uid = "0190a000-0000-7000-8000-000000000001";
    let (ia, _) = begin(&a.db, Begin { source: &src, parser: "t", blob: None, uid: Some(uid.into()), candidates: None }, T0).unwrap();
    finish(&a.db, ia, T0).unwrap();
    begin(&b.db, Begin { source: &src, parser: "t", blob: None, uid: Some(uid.into()), candidates: None }, T0).unwrap();
    let (ia, ib) = (sync::of_profile(&a).unwrap(), sync::of_profile(&b).unwrap());
    assert!(ib.running.contains(uid));
    assert!(sync::wants(&ib, &ia, &|_| false).imports.is_empty());
}

fn stamp(at: i64, by: &str) -> Stamp {
    Stamp { set_at: at << 16, set_by: by.into() }
}

#[test]
fn settings_take_the_larger_stamp_and_refuse_the_future() {
    let mut here = State::default();
    let theirs = Settings {
        profile: Some(ProfileSetting {
            stamp: stamp(T0, "dev-b"),
            birth: Some("1990-05".into()),
            gender: Some("Female".into()),
            ethnicity: None,
        }),
        whoop_stopped: None,
    };
    assert!(sync::apply_settings(&mut here, &theirs, T0).profile);
    assert_eq!(here.profile.as_ref().unwrap().birth.as_deref(), Some("1990-05"));
    // an older one doesn't replace it
    let older = Settings { profile: Some(ProfileSetting { stamp: stamp(T0 - 1, "dev-z"), ..Default::default() }), whoop_stopped: None };
    assert!(!sync::apply_settings(&mut here, &older, T0).profile);
    // two days ahead, or a bad value: refused
    let ahead = Settings {
        profile: Some(ProfileSetting { stamp: stamp(T0 + 2 * 86_400_000, "dev-b"), ..Default::default() }),
        whoop_stopped: None,
    };
    assert!(sync::apply_settings(&mut here, &ahead, T0).refused);
    let bad = Settings {
        profile: Some(ProfileSetting { stamp: stamp(T0 + 1, "dev-b"), birth: Some("1990-13".into()), ..Default::default() }),
        whoop_stopped: None,
    };
    assert!(sync::apply_settings(&mut here, &bad, T0).refused);
    // the web's pattern exactly: no sign, no other digits
    for b in ["2000-+1", "1990-1 ", "2100-01", "1990-00", "١٩٩٠-01"] {
        assert!(!sync::birth_ok(b), "{b}");
    }
    assert!(sync::birth_ok("1999-12"));
    // the lines both sides write agree once applied
    let mut there = State::default();
    sync::apply_settings(&mut there, &sync::settings_of(&here), T0);
    assert_eq!(sync::vault_settings(&there), sync::vault_settings(&here));
}

#[test]
fn a_newer_whoop_stop_disconnects_an_older_connection_only() {
    let stop = |at: i64| Settings { profile: None, whoop_stopped: Some(Stopped { at, stamp: stamp(at, "dev-b") }) };
    let connected = |at: i64| State { whoop: Some(Whoop { connected_at: Some(at), ..Default::default() }), ..Default::default() };
    let mut s = connected(T0 - 10);
    let r = sync::apply_settings(&mut s, &stop(T0), T0);
    assert!(r.whoop_stopped && r.whoop_disconnected && s.whoop.is_none());
    // connected again after the stop: kept
    let mut s = connected(T0 + 10);
    let r = sync::apply_settings(&mut s, &stop(T0), T0 + 20);
    assert!(r.whoop_stopped && !r.whoop_disconnected && s.whoop.is_some());
}

#[test]
fn stamps_cross_json_as_decimal_strings_and_null_fields_mean_absent() {
    // an HLC is above 2^53: a JSON number would be rounded by JS
    let s = Settings { profile: None, whoop_stopped: Some(Stopped { at: T0, stamp: stamp(T0, "dev-b") }) };
    let j = serde_json::to_value(&s).unwrap();
    assert_eq!(j["whoop_stopped"]["set_at"], serde_json::json!((T0 << 16).to_string()));
    assert_eq!(serde_json::from_value::<Settings>(j).unwrap(), s);
    // leading zeros, signs and other text are refused; null fields are absent
    for bad in ["01", "-1", "1e3", "", "99999999999999999999"] {
        let j = serde_json::json!({ "profile": { "set_at": bad, "set_by": "d" } });
        assert!(serde_json::from_value::<Settings>(j).is_err(), "{bad}");
    }
    let j = serde_json::json!({ "profile": { "set_at": "5", "set_by": "d", "birth": null } });
    assert_eq!(serde_json::from_value::<Settings>(j).unwrap().profile.unwrap().birth, None);
}
