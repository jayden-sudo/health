//! Merge and backup rules beyond the web fixture: tombstones, no revival, blob restore, malformed packs.

use health_core::backup;
use health_core::keys::MemoryStore;
use health_core::manual::{self, Entry, When};
use health_core::profile::Profile;
use health_core::write::{Begin, ObsRow, SourceSpec, begin, finish, write};
use health_core::{Value, erase, merge, pack, query, review};

const T: i64 = 1_762_704_000_000;
const PW: &str = "correct horse";

fn weight(v: f64) -> Entry {
    Entry {
        metric: "loinc:29463-7".into(),
        value: Value::number(v),
        unit: Some("kg".into()),
        at: When::At { ms: T, end_ms: None, tz_min: 480 },
    }
}

struct Two {
    dir: tempfile::TempDir,
    keys: MemoryStore,
}

impl Two {
    fn new() -> Two {
        let t = Two { dir: tempfile::tempdir().unwrap(), keys: MemoryStore::default() };
        drop(Profile::init(t.dir.path(), "a", &t.keys).unwrap());
        drop(Profile::init(t.dir.path(), "b", &t.keys).unwrap());
        t
    }
    fn open(&self, name: &str) -> Profile {
        Profile::open(self.dir.path(), name, &self.keys, true, T).unwrap()
    }
    fn export(&self, p: &Profile, file: &str, blobs: bool) -> std::path::PathBuf {
        let out = self.dir.path().join(file);
        backup::export(p, &out, PW, blobs, T, pack::Options { iter: 10, chunk: 4096 }).unwrap();
        out
    }
}

fn values(p: &Profile) -> Vec<Value> {
    query::obs_query(&p.db, &health_core::scope::Scope::all(), "loinc:29463-7", None, None, None, 99)
        .unwrap()
        .into_iter()
        .map(|x| x.value)
        .collect()
}

#[test]
fn a_remote_tombstone_erases_and_an_old_copy_does_not_revive() {
    let t = Two::new();
    let a = t.open("a");
    let id = manual::enter(&a.db, &[weight(70.0)], T).unwrap().import_id;
    let before = t.export(&a, "before.zuhealth", true);
    let b = t.open("b");
    backup::import(&b, &before, PW, T).unwrap();
    assert_eq!(values(&b), vec![Value::number(70.0)]);
    // a deletes it; b learns the tombstone
    erase::erase(&a.db, id, T).unwrap();
    let after = t.export(&a, "after.zuhealth", true);
    assert_eq!(backup::import(&b, &after, PW, T).unwrap().merge.erased_imports, 1);
    assert!(values(&b).is_empty());
    // the old backup again: the tombstone wins
    backup::import(&b, &before, PW, T).unwrap();
    assert!(values(&b).is_empty());
}

#[test]
fn an_edited_then_removed_entry_stays_removed_after_merging_an_older_backup() {
    let t = Two::new();
    let a = t.open("a");
    manual::enter(&a.db, &[weight(70.0)], T).unwrap();
    let old = t.export(&a, "old.zuhealth", true);
    manual::edit(&a.db, "loinc:29463-7", T, &weight(71.0), T).unwrap();
    manual::remove(&a.db, "loinc:29463-7", T, T).unwrap();
    backup::import(&a, &old, PW, T).unwrap();
    assert!(values(&a).is_empty());
}

#[test]
fn blobs_travel_and_are_restored_later_too() {
    let t = Two::new();
    let a = t.open("a");
    let src = t.dir.path().join("photo.jpg");
    std::fs::write(&src, b"not really a jpeg").unwrap();
    let sha = a.blobs.put_file(&src).unwrap();
    let s = SourceSpec::new("ocr", "photo");
    let (id, _) =
        begin(&a.db, Begin { source: &s, parser: "ocr@1", blob: Some(&sha), uid: None, candidates: Some(serde_json::json!({})) }, T)
            .unwrap();
    review::confirm(
        &a.db,
        id,
        &[ObsRow {
            metric: "loinc:2345-7".into(),
            start_ms: T,
            end_ms: None,
            tz_min: 480,
            value: Value::Int(90),
            unit: None,
            detail: None,
        }],
        None,
        &[],
        T,
    )
    .unwrap();
    let b = t.open("b");
    // first without blobs: reported missing
    let lean = t.export(&a, "lean.zuhealth", false);
    assert_eq!(backup::import(&b, &lean, PW, T).unwrap().missing_blobs, vec![sha.clone()]);
    // then the full one restores the file although the import is no longer new
    let full = t.export(&a, "full.zuhealth", true);
    let r = backup::import(&b, &full, PW, T).unwrap();
    assert_eq!((r.restored_blobs, r.missing_blobs.len()), (1, 0));
    let mut out = Vec::new();
    b.blobs.get(&sha, &mut out).unwrap();
    assert_eq!(out, b"not really a jpeg");
}

#[test]
fn text_values_are_not_blobs() {
    let t = Two::new();
    let a = t.open("a");
    let e = Entry {
        metric: "loinc:5778-6".into(),
        value: Value::Text("yellow".into()),
        unit: None,
        at: When::At { ms: T, end_ms: None, tz_min: 0 },
    };
    manual::enter(&a.db, &[e], T).unwrap();
    let f = t.export(&a, "x.zuhealth", true);
    let r = backup::import(&t.open("b"), &f, PW, T).unwrap();
    assert!(r.merge.needed_blobs.is_empty() && r.missing_blobs.is_empty());
}

#[test]
fn a_repeated_entry_is_refused_not_a_panic() {
    let t = Two::new();
    let a = t.open("a");
    let plain = t.dir.path().join("plain.db");
    merge::export_plain(&a.db, &plain).unwrap();
    let f = t.dir.path().join("dup.zuhealth");
    let mut w = pack::Writer::new(std::fs::File::create(&f).unwrap(), PW, pack::Options { iter: 10, chunk: 4096 }).unwrap();
    w.entry("db.sqlite", std::fs::File::open(&plain).unwrap()).unwrap();
    w.entry("db.sqlite", std::fs::File::open(&plain).unwrap()).unwrap();
    w.finish().unwrap();
    assert_eq!(backup::import(&a, &f, PW, T).unwrap_err().code, "backup_dup");
}

#[test]
fn export_never_touches_a_neighbouring_part_file() {
    let t = Two::new();
    let a = t.open("a");
    let part = t.dir.path().join("b.part");
    std::fs::write(&part, b"mine").unwrap();
    std::fs::write(t.dir.path().join("b.zuhealth"), b"exists").unwrap();
    assert_eq!(backup::export(&a, &t.dir.path().join("b.zuhealth"), PW, true, T, Default::default()).unwrap_err().code, "exists");
    t.export(&a, "c.zuhealth", true);
    assert_eq!(std::fs::read(&part).unwrap(), b"mine");
}

#[test]
fn unreferenced_blob_files_are_swept_when_a_writer_opens() {
    let t = Two::new();
    let a = t.open("a");
    let src = t.dir.path().join("f");
    std::fs::write(&src, b"orphan").unwrap();
    let sha = a.blobs.put_file(&src).unwrap();
    drop(a);
    assert!(!t.open("a").blobs.has(&sha));
}

#[test]
fn times_beyond_safe_integers_are_rejected() {
    let c = health_core::db::open_memory().unwrap();
    let s = SourceSpec::new("x", "y");
    let (id, _) = begin(&c, Begin { source: &s, parser: "p", blob: None, uid: None, candidates: None }, T).unwrap();
    let r =
        |t| ObsRow { metric: "loinc:8867-4".into(), start_ms: t, end_ms: None, tz_min: 0, value: Value::Int(60), unit: None, detail: None };
    let w = write(&c, id, &[r(1_795_176_000_000_000_000), r(i64::MAX)]).unwrap();
    assert_eq!((w.written, w.rejected.get("time").copied()), (0, Some(2)));
    finish(&c, id, T).unwrap();
}

#[test]
fn a_backup_with_other_schema_comments_still_merges() {
    let t = Two::new();
    let a = t.open("a");
    manual::enter(&a.db, &[weight(70.0)], T).unwrap();
    let plain = t.dir.path().join("plain.db");
    merge::export_plain(&a.db, &plain).unwrap();
    // rebuild the plain file from schema text whose comments differ, as an older web version would have
    let other = t.dir.path().join("other.db");
    let c = health_core::rusqlite::Connection::open(&other).unwrap();
    c.execute_batch(&health_core::db::schema_body().replace("-- ", "-- older wording ")).unwrap();
    c.execute("ATTACH ? AS src", [plain.to_str().unwrap()]).unwrap();
    for tb in ["metric", "source", "import", "observation"] {
        c.execute(&format!("INSERT INTO main.{tb} SELECT * FROM src.{tb}"), []).unwrap();
    }
    c.execute_batch("DETACH src").unwrap();
    drop(c);
    let b = t.open("b");
    assert_eq!(merge::merge_from(&b.db, &other, T).unwrap().added_imports, 1);
}

#[test]
fn a_crafted_backup_cannot_take_over_local_rows() {
    let t = Two::new();
    let a = t.open("a");
    manual::enter(&a.db, &[weight(70.0)], T).unwrap();
    // attacker: same manual point, uid that sorts above everything, correction from the far future
    let plain = t.dir.path().join("evil.db");
    merge::export_plain(&a.db, &plain).unwrap();
    let c = health_core::rusqlite::Connection::open(&plain).unwrap();
    c.execute_batch(
        "UPDATE import SET uid = 'zzzz';
         UPDATE observation SET value = 999;
         INSERT INTO correction SELECT metric_id, source_id, start_ms, 9223372036854775807, NULL FROM observation;",
    )
    .unwrap();
    drop(c);
    let r = merge::merge_from(&a.db, &plain, T).unwrap();
    assert_eq!((r.refused_imports, r.refused_corrections), (1, 1));
    assert_eq!(values(&a), vec![Value::number(70.0)]);
}

fn glucose(v: i64) -> ObsRow {
    ObsRow { metric: "loinc:2345-7".into(), start_ms: T, end_ms: None, tz_min: 480, value: Value::Int(v), unit: None, detail: None }
}

fn photo_import(p: &Profile, sha: &str, v: i64) -> i64 {
    let s = SourceSpec::new("ocr", "photo");
    let (id, _) =
        begin(&p.db, Begin { source: &s, parser: "ocr@1", blob: Some(sha), uid: None, candidates: Some(serde_json::json!({})) }, T)
            .unwrap();
    review::confirm(&p.db, id, &[glucose(v)], None, &[], T).unwrap().import_id
}

#[test]
fn a_blob_erased_and_referenced_again_in_one_merge_is_kept() {
    let t = Two::new();
    let a = t.open("a");
    let b = t.open("b");
    let sha = a.blobs.put(&b"report bytes"[..]).unwrap();
    let x = photo_import(&a, &sha, 90);
    backup::import(&b, &t.export(&a, "one.zuhealth", true), PW, T).unwrap();
    // a erases the import and imports the same photo again
    let r = erase::erase(&a.db, x, T).unwrap();
    a.blobs.remove(&r.orphan_blobs);
    a.blobs.put(&b"report bytes"[..]).unwrap();
    photo_import(&a, &sha, 91);
    let r = backup::import(&b, &t.export(&a, "two.zuhealth", false), PW, T).unwrap();
    assert!(erase::referenced(&b.db, &sha).unwrap());
    assert!(r.merge.orphan_blobs.is_empty(), "{:?}", r.merge.orphan_blobs);
    assert!(b.blobs.has(&sha), "b's copy of a still-referenced photo was deleted");
}

#[test]
fn an_entry_entered_twice_then_removed_stays_removed_everywhere() {
    let t = Two::new();
    let a = t.open("a");
    let b = t.open("b");
    manual::enter(&a.db, &[weight(70.0)], T).unwrap();
    let old = t.export(&a, "old.zuhealth", true);
    backup::import(&b, &old, PW, T).unwrap();
    manual::enter(&a.db, &[weight(71.0)], T).unwrap();
    manual::remove(&a.db, "loinc:29463-7", T, T).unwrap();
    assert!(values(&a).is_empty());
    backup::import(&b, &t.export(&a, "a2.zuhealth", true), PW, T).unwrap();
    assert!(values(&b).is_empty(), "the delete didn't reach b");
    backup::import(&a, &t.export(&b, "b2.zuhealth", true), PW, T).unwrap();
    assert!(values(&a).is_empty(), "b brought the old value back");
    backup::import(&a, &old, PW, T).unwrap();
    assert!(values(&a).is_empty(), "an older backup brought the old value back");
}

#[test]
fn export_replaces_a_file_only_when_asked() {
    let t = Two::new();
    let a = t.open("a");
    let out = t.dir.path().join("b.zuhealth");
    std::fs::write(&out, b"old").unwrap();
    let opts = pack::Options { iter: 10, chunk: 4096 };
    assert_eq!(backup::export(&a, &out, PW, true, T, pack::Options { iter: 10, chunk: 4096 }).unwrap_err().code, "exists");
    backup::export_to(&a, &out, PW, true, true, T, opts).unwrap();
    assert_ne!(std::fs::read(&out).unwrap(), b"old");
}
