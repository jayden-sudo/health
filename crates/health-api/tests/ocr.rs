//! import.ocr / review.confirm_ocr rules at the op level (no models: the recognition result is given).

use health_api::{Caller, Ctx, call};
use health_core::keys::MemoryStore;
use health_core::profile::Profile;
use serde_json::{Value, json};

const NOW: i64 = 1_795_176_000_000;

fn setup() -> (tempfile::TempDir, Profile, std::path::PathBuf) {
    let d = tempfile::tempdir().unwrap();
    let p = Profile::init(d.path(), "default", &MemoryStore::default()).unwrap();
    let file = d.path().join("report.png");
    std::fs::write(&file, b"not really a png, the result is given").unwrap();
    (d, p, file)
}

fn recognized(date: Option<&str>) -> Value {
    json!({ "items": [{ "name": "葡萄糖", "v": 5.5, "unit": "mmol/L", "text": "葡萄糖 5.5 mmol/L" }], "text": "report", "date": date })
}

fn code(r: Result<health_api::Outcome, health_core::Error>) -> String {
    match r {
        Ok(_) => "ok".into(),
        Err(e) => e.code.to_string(),
    }
}

#[test]
fn an_impossible_date_on_the_page_means_undated_and_then_a_date_is_required() {
    let (_d, p, file) = setup();
    let mut ctx = Ctx::new(&p, NOW, Caller::Owner);
    let r =
        call(&mut ctx, "import.ocr", json!({ "path": file, "tz_min": 480, "recognized": recognized(Some("2026-02-31")) })).unwrap().value;
    let id = r["import_id"].clone();
    let batch = &call(&mut ctx, "review.list", json!({})).unwrap().value[0];
    assert_eq!(batch["candidates"]["dated"], false);
    let choices = json!([{ "code": "loinc:2345-7" }]);
    assert_eq!(code(call(&mut ctx, "review.confirm_ocr", json!({ "id": id, "choices": choices, "tz_min": 480 }))), "date_required");
    assert_eq!(
        code(call(&mut ctx, "review.confirm_ocr", json!({ "id": id, "choices": choices, "tz_min": 480, "date": "+2026-3-7" }))),
        "bad_date"
    );
    assert_eq!(
        code(call(&mut ctx, "review.confirm_ocr", json!({ "id": id, "choices": choices, "tz_min": 99_999, "date": "2026-03-07" }))),
        "bad_tz"
    );
    assert_eq!(
        code(call(&mut ctx, "review.confirm_ocr", json!({ "id": id, "choices": choices, "tz_min": 480, "date": "2026-03-07" }))),
        "ok"
    );
}

#[test]
fn a_refused_import_leaves_no_file_and_a_dry_run_is_refused() {
    let (_d, p, file) = setup();
    let blobs = || std::fs::read_dir(p.dir.join("blobs")).map(|r| r.count()).unwrap_or(0);
    let mut ctx = Ctx::new(&p, NOW, Caller::Owner);
    ctx.dry_run = true;
    assert_eq!(code(call(&mut ctx, "import.ocr", json!({ "path": file, "tz_min": 480, "recognized": recognized(None) }))), "no_dry_run");
    assert_eq!(blobs(), 0);
    let mut ctx = Ctx::new(&p, NOW, Caller::Owner);
    assert_eq!(
        code(call(&mut ctx, "import.ocr", json!({ "path": file, "tz_min": 480, "recognized": json!({ "items": [], "text": "" }) }))),
        "empty"
    );
    assert_eq!(blobs(), 0, "a refused import keeps no copy of the file");
    assert_eq!(code(call(&mut ctx, "import.ocr", json!({ "path": file, "tz_min": 480, "recognized": recognized(None) }))), "ok");
    assert_eq!(blobs(), 1);
    assert_eq!(code(call(&mut ctx, "import.ocr", json!({ "path": file, "tz_min": 480, "recognized": recognized(None) }))), "blob_dup");
    assert_eq!(blobs(), 1);
}
