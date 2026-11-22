//! Manual entry, edit, remove; uid ordering with a pinned clock.

use health_core::db;
use health_core::manual::{self, Entry, When};
use health_core::query;
use health_core::value::Value;

const T: i64 = 1_762_704_000_000;

fn weight(v: f64, unit: Option<&str>, at: When) -> Entry {
    Entry { metric: "loinc:29463-7".into(), value: Value::number(v), unit: unit.map(String::from), at }
}

#[test]
fn units_are_required_and_converted() {
    let c = db::open_memory().unwrap();
    let at = || When::At { ms: T, end_ms: None, tz_min: 480 };
    assert_eq!(manual::enter(&c, &[weight(70.0, None, at())], T).unwrap_err().code, "unit_required");
    assert_eq!(manual::enter(&c, &[weight(70.0, Some("mmol/L"), at())], T).unwrap_err().code, "unit_unknown");
    manual::enter(&c, &[weight(1000.0, Some("g"), at())], T).unwrap();
    assert_eq!(query::latest(&c, &health_core::scope::Scope::all(), None).unwrap()[0].value, Value::Int(1));
}

#[test]
fn date_only_entries_sit_at_local_midnight() {
    let c = db::open_memory().unwrap();
    manual::enter(&c, &[weight(70.0, Some("kg"), When::Date { date: "2025-11-10".into(), tz_min: 480 })], T).unwrap();
    let p = &query::latest(&c, &health_core::scope::Scope::all(), None).unwrap()[0];
    assert_eq!(p.start_ms, T);
    assert_eq!(p.detail.as_ref().unwrap()["precision"], "day");
}

#[test]
fn edits_win_even_with_a_pinned_clock() {
    let c = db::open_memory().unwrap();
    let at = || When::At { ms: T, end_ms: None, tz_min: 480 };
    manual::enter(&c, &[weight(70.0, Some("kg"), at())], T).unwrap();
    for v in [71.0, 72.0, 73.0] {
        let r = manual::edit(&c, "loinc:29463-7", T, &weight(v, Some("kg"), at()), T).unwrap();
        assert_eq!((r.result.written, r.result.kept), (1, 0));
    }
    assert_eq!(query::latest(&c, &health_core::scope::Scope::all(), None).unwrap()[0].value, Value::Int(73));
    // moving the entry in time removes the old point
    manual::edit(&c, "loinc:29463-7", T, &weight(74.0, Some("kg"), When::At { ms: T + 1, end_ms: None, tz_min: 480 }), T).unwrap();
    let pts = query::obs_query(&c, &health_core::scope::Scope::all(), "loinc:29463-7", None, None, None, 9).unwrap();
    assert_eq!((pts.len(), pts[0].start_ms), (1, T + 1));
    manual::remove(&c, "loinc:29463-7", T + 1, T).unwrap();
    assert!(query::latest(&c, &health_core::scope::Scope::all(), None).unwrap().is_empty());
    assert_eq!(manual::remove(&c, "loinc:29463-7", T + 1, T).unwrap_err().code, "no_row");
}
