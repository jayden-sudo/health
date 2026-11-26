//! The owner's "Clear data" (profile::wipe): the database first, nothing deleted while another process writes,
//! and the next init a fresh install with a new key and none of the old values.

use health_core::keys::MemoryStore;
use health_core::profile::{self, Profile};
use health_core::write::{Begin, ObsRow, SourceSpec, begin, finish, write};
use health_core::{Value, query};

const T0: i64 = 1_795_000_000_000;

fn add(p: &Profile) {
    let src = SourceSpec::new("manual", "");
    let (id, _) = begin(&p.db, Begin { source: &src, parser: "manual", blob: None, uid: None, candidates: None }, T0).unwrap();
    let row = ObsRow {
        metric: "loinc:29463-7".into(),
        start_ms: T0,
        end_ms: None,
        tz_min: 480,
        value: Value::Real(70.2),
        unit: None,
        detail: None,
    };
    write(&p.db, id, &[row]).unwrap();
    finish(&p.db, id, T0).unwrap();
}

#[test]
fn clearing_deletes_the_profile_and_the_next_init_is_a_fresh_install() {
    let d = tempfile::tempdir().unwrap();
    let keys = MemoryStore::default();
    let p = Profile::init(d.path(), "a", &keys).unwrap();
    add(&p);
    let device = p.vault.device_id().unwrap();
    let old = p.id.clone();
    drop(p);
    assert_eq!(profile::wipe(d.path(), "a").unwrap().as_deref(), Some(old.as_str()), "the id comes back for the key store");
    assert!(!d.path().join("profiles/a").exists());
    // the next start finds no profile (the app's first run), and init gives a new id, key and device
    assert!(Profile::open(d.path(), "a", &keys, true, T0).is_err());
    let p = Profile::init(d.path(), "a", &keys).unwrap();
    assert_ne!(p.id, old);
    assert_ne!(p.vault.device_id().unwrap(), device);
    assert!(query::latest(&p.db, &health_core::scope::Scope::all(), None).unwrap().is_empty());
    // nothing to clear is not an error
    drop(p);
    profile::wipe(d.path(), "a").unwrap();
    assert_eq!(profile::wipe(d.path(), "a").unwrap(), None);
}

#[test]
fn while_another_process_writes_nothing_is_deleted_and_the_profile_still_opens() {
    let d = tempfile::tempdir().unwrap();
    let keys = MemoryStore::default();
    let p = Profile::init(d.path(), "a", &keys).unwrap();
    add(&p);
    let e = profile::wipe(d.path(), "a").unwrap_err();
    assert_eq!(e.code, "busy");
    for f in ["id", "health.db", "vault", "access.db"] {
        assert!(p.dir.join(f).exists() || f == "vault", "{f} kept");
    }
    drop(p);
    let p = Profile::open(d.path(), "a", &keys, true, T0).unwrap();
    assert_eq!(query::latest(&p.db, &health_core::scope::Scope::all(), None).unwrap().len(), 1);
}

#[test]
fn a_database_that_no_longer_opens_can_still_be_cleared() {
    let d = tempfile::tempdir().unwrap();
    let keys = MemoryStore::default();
    let p = Profile::init(d.path(), "a", &keys).unwrap();
    let dir = p.dir.clone();
    drop(p);
    // its key lost: the database can't be opened any more
    let lost = MemoryStore::default();
    assert!(Profile::open(d.path(), "a", &lost, true, T0).is_err());
    std::fs::write(dir.join("health.db"), b"not a database").unwrap();
    profile::wipe(d.path(), "a").unwrap();
    Profile::init(d.path(), "a", &lost).unwrap();
}

#[test]
fn init_drops_what_an_earlier_profile_left_under_its_lost_key() {
    let d = tempfile::tempdir().unwrap();
    let p = Profile::init(d.path(), "a", &MemoryStore::default()).unwrap();
    p.vault.device_id().unwrap();
    let dir = p.dir.clone();
    drop(p);
    // the database gone (an interrupted clear), the rest left under a key this machine no longer has
    std::fs::remove_file(dir.join("health.db")).unwrap();
    let keys = MemoryStore::default();
    let p = Profile::init(d.path(), "a", &keys).unwrap();
    // the vault opens under the new key: the old one was dropped, not read
    p.vault.device_id().unwrap();
    assert!(p.vault.load().is_ok());
}
