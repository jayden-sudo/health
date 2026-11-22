//! DB design §8 as 24 numbered checks (decision 12). Each test name starts with its number.

use health_core::correction::{self, Key};
use health_core::db::{self, Db};
use health_core::value::Value;
use health_core::write::{Begin, ObsRow, SourceSpec, begin, finish, write};
use health_core::{erase, merge, query, review};

const H: i64 = 3_600_000;
/// 2025-11-10 00:00 at +08:00
const T0: i64 = 1_762_704_000_000;
const TZ: i64 = 480;
const NOW: i64 = T0 + 10 * 24 * H;

fn row(metric: &str, t: i64, v: Value) -> ObsRow {
    ObsRow { metric: metric.into(), start_ms: t, end_ms: None, tz_min: TZ, value: v, unit: None, detail: None }
}
fn num(x: f64) -> Value {
    Value::number(x)
}
fn src(connector: &str, ext: &str) -> SourceSpec {
    SourceSpec::new(connector, ext)
}

/// One finished import of `rows` from `s`.
fn import(c: &Db, s: &SourceSpec, uid: &str, blob: Option<&str>, rows: &[ObsRow]) -> i64 {
    let (id, _) = begin(c, Begin { source: s, parser: "test@1", blob, uid: Some(uid.into()), candidates: None }, NOW).unwrap();
    write(c, id, rows).unwrap();
    finish(c, id, NOW).unwrap();
    id
}

fn review_import(c: &Db, uid: &str, blob: &str) -> i64 {
    let s = src("ocr", "hospital:a");
    begin(
        c,
        Begin { source: &s, parser: "ocr@1", blob: Some(blob), uid: Some(uid.into()), candidates: Some(serde_json::json!({"items": []})) },
        NOW,
    )
    .unwrap()
    .0
}

fn count(c: &Db, sql: &str) -> i64 {
    c.query_row(sql, [], |r| r.get(0)).unwrap()
}

fn mid(c: &Db, code: &str) -> i64 {
    c.query_row("SELECT id FROM metric WHERE code = ?", [code], |r| r.get(0)).unwrap()
}
fn sid(c: &Db, connector: &str, ext: &str) -> i64 {
    c.query_row("SELECT id FROM source WHERE connector = ? AND external_id = ?", [connector, ext], |r| r.get(0)).unwrap()
}

fn add_metric(c: &Db, code: &str, unit: &str, agg: &str) {
    c.execute("INSERT INTO metric (code, name, unit, agg) VALUES (?, ?, ?, ?)", [code, code, unit, agg]).unwrap();
}

const GLU: &str = "loinc:2345-7"; // blood glucose, mg/dL
const UGLU: &str = "loinc:25428-4"; // glucose presence in urine (text)
const HR: &str = "loinc:8867-4";
const STEPS: &str = "loinc:55423-8";
const DEEP: &str = "loinc:93831-6";

#[test]
fn c01_review_rows_are_invisible() {
    let c = db::open_memory().unwrap();
    review_import(&c, "u1", &"a".repeat(64));
    assert_eq!(count(&c, "SELECT count(*) FROM v_observation"), 0);
    assert_eq!(count(&c, "SELECT count(*) FROM v_daily"), 0);
    assert!(query::latest(&c, &health_core::scope::Scope::all(), None).unwrap().is_empty());
}

#[test]
fn c02_confirmed_rows_are_visible_and_converted() {
    let c = db::open_memory().unwrap();
    let id = review_import(&c, "u1", &"a".repeat(64));
    let mut r = row(GLU, T0, num(5.3));
    r.unit = Some("mmol/L".into());
    review::confirm(&c, id, &[r], Some("空腹血糖 5.3 mmol/L"), &[], NOW).unwrap();
    let p = query::obs_query(&c, &health_core::scope::Scope::all(), GLU, None, None, None, 10).unwrap();
    assert_eq!(p.len(), 1);
    assert!((p[0].value.as_f64().unwrap() - 95.4827).abs() < 1e-3);
}

#[test]
fn c03_alias_name_and_unit_map_to_blood_glucose() {
    let c = db::open_memory().unwrap();
    review::add_alias(&c, &review::Alias { name: "葡萄糖".into(), unit: "mmol/L".into(), panel: None, code: GLU.into() }).unwrap();
    assert_eq!(review::lookup_alias(&c, "葡 萄 糖", "MMOL/L", None).unwrap().as_deref(), Some(GLU));
}

#[test]
fn c04_panel_alias_maps_urine_glucose_first() {
    let c = db::open_memory().unwrap();
    review::add_alias(&c, &review::Alias { name: "葡萄糖".into(), unit: "mmol/L".into(), panel: None, code: GLU.into() }).unwrap();
    review::add_alias(&c, &review::Alias { name: "葡萄糖".into(), unit: "mmol/L".into(), panel: Some("尿常規".into()), code: UGLU.into() })
        .unwrap();
    assert_eq!(review::lookup_alias(&c, "葡萄糖", "mmol/L", Some("尿常規")).unwrap().as_deref(), Some(UGLU));
}

#[test]
fn c05_name_without_unit_or_panel_stays_in_review() {
    let c = db::open_memory().unwrap();
    review::add_alias(&c, &review::Alias { name: "葡萄糖".into(), unit: "mmol/L".into(), panel: None, code: GLU.into() }).unwrap();
    assert_eq!(review::lookup_alias(&c, "葡萄糖", "", None).unwrap(), None);
}

#[test]
fn c06_chinese_full_text_search() {
    let c = db::open_memory().unwrap();
    let id = review_import(&c, "u1", &"a".repeat(64));
    let id =
        review::confirm(&c, id, &[row(GLU, T0, num(90.0))], Some("影像所見：肝臟大小正常。結論：未見異常"), &[], NOW).unwrap().import_id;
    let hits = query::report_search(&c, &health_core::scope::Scope::all(), "未見異常", 10).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].import_id, id);
    assert!(query::report_search(&c, &health_core::scope::Scope::all(), "肝臟", 10).unwrap().len() == 1); // two characters: LIKE path
}

#[test]
fn c07_same_file_twice_is_refused() {
    let c = db::open_memory().unwrap();
    import(&c, &src("apple_export", "x"), "u1", Some(&"b".repeat(64)), &[]);
    let e =
        begin(&c, Begin { source: &src("apple_export", "x"), parser: "p", blob: Some(&"b".repeat(64)), uid: None, candidates: None }, NOW)
            .unwrap_err();
    assert_eq!(e.code, "blob_dup");
}

#[test]
fn c08_reimport_is_idempotent() {
    let c = db::open_memory().unwrap();
    let rows: Vec<ObsRow> = (0..5).map(|i| row(HR, T0 + i * H, num(60.0 + i as f64))).collect();
    let s = src("healthkit", "watch:1");
    import(&c, &s, "u1", None, &rows);
    let snap = |c: &Db| {
        c.prepare("SELECT metric_id, start_ms, value FROM v_observation ORDER BY 2")
            .unwrap()
            .query_map([], |r| Ok((r.get::<_, i64>(1)?, r.get::<_, Value>(2)?)))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    };
    let before = snap(&c);
    import(&c, &s, "u2", None, &rows);
    assert_eq!(before, snap(&c));
    assert_eq!(count(&c, "SELECT count(*) FROM observation"), 5);
}

#[test]
fn c09_source_revision_overwrites() {
    let c = db::open_memory().unwrap();
    let s = src("dexcom", "g7:1");
    import(&c, &s, "u1", None, &[row(GLU, T0, num(100.0))]);
    import(&c, &s, "u2", None, &[row(GLU, T0, num(104.0))]);
    assert_eq!(query::obs_query(&c, &health_core::scope::Scope::all(), GLU, None, None, None, 9).unwrap()[0].value, num(104.0));
    // an older delivery arriving late does not win
    let (id, _) = begin(&c, Begin { source: &s, parser: "p", blob: None, uid: Some("u0".into()), candidates: None }, NOW).unwrap();
    let w = write(&c, id, &[row(GLU, T0, num(1.0))]).unwrap();
    assert_eq!((w.written, w.kept), (0, 1));
}

#[test]
fn c10_user_delete_survives_a_source_repush() {
    let c = db::open_memory().unwrap();
    let s = src("healthkit", "watch:1");
    import(&c, &s, "u1", None, &[row(HR, T0, num(190.0))]);
    let k = Key { metric_id: mid(&c, HR), source_id: sid(&c, "healthkit", "watch:1"), start_ms: T0 };
    correction::correct(&c, k, &Value::Null, NOW).unwrap();
    import(&c, &s, "u2", None, &[row(HR, T0, num(190.0))]);
    assert!(query::obs_query(&c, &health_core::scope::Scope::all(), HR, None, None, None, 9).unwrap().is_empty());
    assert_eq!(count(&c, "SELECT count(*) FROM daily"), 0);
}

#[test]
fn c11_censored_values_skip_daily_and_keep_their_comparator() {
    let c = db::open_memory().unwrap();
    let mut a = row(GLU, T0 + H, num(2.0));
    a.detail = Some(serde_json::json!({"comparator": "<"}));
    import(&c, &src("lab", "a"), "u1", None, &[a, row(GLU, T0 + 2 * H, num(90.0))]);
    let d = query::daily(&c, &health_core::scope::Scope::all(), GLU, None, None).unwrap();
    assert_eq!((d[0].n, d[0].min), (1, Some(90.0)));
    let p = query::obs_query(&c, &health_core::scope::Scope::all(), GLU, None, None, None, 9).unwrap();
    assert_eq!(p[0].comparator.as_deref(), Some("<"));
}

#[test]
fn c12_overlapping_sleep_is_a_union() {
    let c = db::open_memory().unwrap();
    let mut a = row(DEEP, T0 - 2 * H, Value::Null);
    a.end_ms = Some(T0);
    let mut b = row(DEEP, T0 - H, Value::Null);
    b.end_ms = Some(T0 + H / 2);
    import(&c, &src("healthkit", "ring"), "u1", None, &[a, b]);
    let d = query::daily(&c, &health_core::scope::Scope::all(), DEEP, None, None).unwrap();
    assert_eq!((d[0].day.as_str(), d[0].seconds), ("2025-11-10", Some(9000)));
}

fn ha_room(c: &Db) {
    add_metric(c, "x:room_co2", "ppm", "level");
    let rows = [
        row("x:room_co2", T0 - 2 * H, num(600.0)), // value at midnight comes from before it
        row("x:room_co2", T0 + 10 * H, num(900.0)),
        row("x:room_co2", T0 + 20 * H, Value::Null), // gateway lost the sensor for an hour
        row("x:room_co2", T0 + 21 * H, num(700.0)),
    ];
    let (id, _) = begin(
        c,
        Begin { source: &src("home_assistant", "ha/sensor.co2"), parser: "ha@1", blob: None, uid: Some("u1".into()), candidates: None },
        NOW,
    )
    .unwrap();
    write(c, id, &rows).unwrap();
    finish(c, id, T0 + 48 * H).unwrap();
}

#[test]
fn c13_level_is_time_weighted_and_skips_the_gap() {
    let c = db::open_memory().unwrap();
    ha_room(&c);
    let d = query::daily(&c, &health_core::scope::Scope::all(), "x:room_co2", Some("2025-11-10"), Some("2025-11-10")).unwrap();
    assert_eq!(d[0].seconds, Some(23 * 3600));
    let mean = (600.0 * 10.0 + 900.0 * 10.0 + 700.0 * 3.0) / 23.0;
    assert!((d[0].mean.unwrap() - mean).abs() < 1e-9);
}

#[test]
fn c14_level_without_change_carries_the_whole_next_day() {
    let c = db::open_memory().unwrap();
    ha_room(&c);
    let d = query::daily(&c, &health_core::scope::Scope::all(), "x:room_co2", Some("2025-11-11"), Some("2025-11-11")).unwrap();
    assert_eq!((d[0].seconds, d[0].mean), (Some(86_400), Some(700.0)));
}

#[test]
fn c15_one_source_per_day_never_a_sum() {
    let c = db::open_memory().unwrap();
    let step = |t, v| {
        let mut r = row(STEPS, t, num(v));
        r.end_ms = Some(t + H);
        r
    };
    import(&c, &SourceSpec { priority: Some(10), ..src("healthkit", "watch:b") }, "u1", None, &[step(T0 + 9 * H, 4000.0)]);
    import(&c, &SourceSpec { priority: Some(10), ..src("healthkit", "watch:a") }, "u2", None, &[step(T0 + 9 * H, 5000.0)]);
    import(&c, &SourceSpec { priority: Some(20), ..src("healthkit", "phone") }, "u3", None, &[step(T0 + 9 * H, 6000.0)]);
    let d = query::daily(&c, &health_core::scope::Scope::all(), STEPS, None, None).unwrap();
    assert_eq!(d.len(), 1);
    assert_eq!((d[0].source_id, d[0].sum), (sid(&c, "healthkit", "watch:b"), Some(4000.0)));
}

#[test]
fn c16_erase_removes_rows_and_full_text() {
    let c = db::open_memory().unwrap();
    let id = review_import(&c, "u1", &"a".repeat(64));
    let id = review::confirm(&c, id, &[row(GLU, T0, num(90.0))], Some("結論：未見異常"), &[], NOW).unwrap().import_id;
    let r = erase::erase(&c, id, NOW).unwrap();
    assert_eq!(r.deleted, 1);
    assert_eq!(
        count(&c, "SELECT count(*) FROM observation")
            + count(&c, "SELECT count(*) FROM report_text")
            + count(&c, "SELECT count(*) FROM daily"),
        0
    );
    assert_eq!(r.orphan_blobs, vec!["a".repeat(64)]);
}

#[test]
fn c17_erased_import_is_a_tombstone_after_the_sync_cursor() {
    let c = db::open_memory().unwrap();
    let a = import(&c, &src("healthkit", "w"), "u1", Some(&"c".repeat(64)), &[row(HR, T0, num(60.0))]);
    import(&c, &src("healthkit", "w"), "u2", None, &[row(HR, T0 + H, num(61.0))]);
    let cursor = count(&c, "SELECT max(id) FROM import");
    erase::erase(&c, a, NOW).unwrap();
    let (id, status, blob, meta): (i64, String, Option<String>, Option<String>) = c
        .query_row("SELECT id, status, blob, meta FROM import WHERE uid = 'u1'", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap();
    assert!(id > cursor);
    assert_eq!((status.as_str(), blob, meta), ("erased", None, None));
}

#[test]
fn c18_blob_still_referenced_is_kept() {
    let c = db::open_memory().unwrap();
    add_metric(&c, "x:eeg_chunk", "", "none");
    let sha = "d".repeat(64);
    let a = import(&c, &src("eeg", "cap"), "u1", Some(&sha), &[]);
    import(&c, &src("eeg", "cap"), "u2", None, &[row("x:eeg_chunk", T0, Value::Text(sha.clone()))]);
    assert!(erase::erase(&c, a, NOW).unwrap().orphan_blobs.is_empty());
    assert!(erase::referenced(&c, &sha).unwrap());
}

#[test]
fn c19_confirm_renumbers_and_a_referenced_import_cannot_be() {
    let c = db::open_memory().unwrap();
    let r = review_import(&c, "u1", &"a".repeat(64));
    let done = import(&c, &src("lab", "x"), "u2", None, &[row(GLU, T0, num(90.0))]);
    let id = review::confirm(&c, r, &[row(GLU, T0 + H, num(91.0))], None, &[], NOW).unwrap().import_id;
    assert!(id > done && id != r);
    let e = c.execute("UPDATE import SET id = 999 WHERE id = ?", [done]);
    assert!(e.is_err(), "foreign key must refuse renumbering an import with rows");
}

#[test]
fn c20_corrections_converge_whatever_the_order() {
    let k_of = |c: &Db| Key { metric_id: mid(c, HR), source_id: sid(c, "healthkit", "w"), start_ms: T0 };
    let mut results = Vec::new();
    for order in [[1i64, 2], [2, 1]] {
        let c = db::open_memory().unwrap();
        import(&c, &src("healthkit", "w"), "u1", None, &[row(HR, T0, num(60.0))]);
        for at in order {
            correction::put(&c, k_of(&c), at << 16, &num(70.0 + at as f64), NOW).unwrap();
        }
        results.push(query::obs_query(&c, &health_core::scope::Scope::all(), HR, None, None, None, 1).unwrap()[0].value.clone());
    }
    assert_eq!(results, vec![num(72.0), num(72.0)]);
}

#[test]
fn c21_on_equal_at_a_value_beats_a_delete() {
    let c = db::open_memory().unwrap();
    import(&c, &src("healthkit", "w"), "u1", None, &[row(HR, T0, num(60.0))]);
    let k = Key { metric_id: mid(&c, HR), source_id: sid(&c, "healthkit", "w"), start_ms: T0 };
    correction::put(&c, k, 5 << 16, &num(65.0), NOW).unwrap();
    assert!(!correction::put(&c, k, 5 << 16, &Value::Null, NOW).unwrap());
    assert_eq!(query::obs_query(&c, &health_core::scope::Scope::all(), HR, None, None, None, 1).unwrap()[0].value, num(65.0));
}

#[test]
fn c22_two_devices_pulling_the_same_point_converge() {
    let dir = tempfile::tempdir().unwrap();
    let x = db::open_memory().unwrap();
    let y = db::open_memory().unwrap();
    let s = src("dexcom", "g7:1");
    import(&x, &s, "0193a5c2-0000-7000-8000-000000000001", None, &[row(GLU, T0, num(100.0))]);
    import(&y, &s, "0193a5c2-0000-7000-8000-000000000002", None, &[row(GLU, T0, num(101.0))]);
    let (px, py) = (dir.path().join("x.db"), dir.path().join("y.db"));
    merge::export_plain(&x, &px).unwrap();
    merge::export_plain(&y, &py).unwrap();
    merge::merge_from(&x, &py, NOW).unwrap();
    merge::merge_from(&y, &px, NOW).unwrap();
    for c in [&x, &y] {
        assert_eq!(query::obs_query(c, &health_core::scope::Scope::all(), GLU, None, None, None, 9).unwrap()[0].value, num(101.0));
    }
}

#[test]
fn c23_crash_mid_import_leaves_nothing() {
    let c = db::open_memory().unwrap();
    let (id, _) =
        begin(&c, Begin { source: &src("apple_export", "x"), parser: "p", blob: None, uid: Some("u1".into()), candidates: None }, NOW)
            .unwrap();
    write(&c, id, &[row(HR, T0, num(60.0)), row(HR, T0 + H, num(61.0))]).unwrap();
    // process dies here; next writer open:
    db::recover_crashed(&c, NOW).unwrap();
    assert_eq!(count(&c, "SELECT count(*) FROM observation"), 0);
    assert_eq!(count(&c, "SELECT count(*) FROM import WHERE status = 'erased'"), 1);
}

#[test]
fn c24_overlapping_exports_belong_to_the_newest() {
    let rows = |n: i64| (0..n).map(|i| row(HR, T0 + i * H, num(60.0 + i as f64))).collect::<Vec<_>>();
    let s = src("apple_export", "iphone");
    // delete the older export: the newer one's 5 rows stay
    let c = db::open_memory().unwrap();
    let old = import(&c, &s, "u1", Some(&"1".repeat(64)), &rows(3));
    import(&c, &s, "u2", Some(&"2".repeat(64)), &rows(5));
    erase::erase(&c, old, NOW).unwrap();
    assert_eq!(count(&c, "SELECT count(*) FROM v_observation"), 5);
    // delete the newer export: everything goes (it owns the overlap)
    let c = db::open_memory().unwrap();
    import(&c, &s, "u1", Some(&"1".repeat(64)), &rows(3));
    let new = import(&c, &s, "u2", Some(&"2".repeat(64)), &rows(5));
    erase::erase(&c, new, NOW).unwrap();
    assert_eq!(count(&c, "SELECT count(*) FROM v_observation"), 0);
}
