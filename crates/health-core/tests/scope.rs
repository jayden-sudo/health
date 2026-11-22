//! A grant's scope limits every read in SQL: metrics outside its groups and time outside its window never come back.

use health_core::Value;
use health_core::access::{GrantSpec, Window, WriteLevel};
use health_core::db;
use health_core::query;
use health_core::scope::Scope;
use health_core::write::{Begin, ObsRow, SourceSpec, begin, finish, write};

const DAY: i64 = 86_400_000;
const NOW: i64 = 1_795_176_000_000;
const RHR: &str = "loinc:40443-4";
const GLU: &str = "loinc:2345-7";

fn seeded() -> db::Db {
    let c = db::open_memory().unwrap();
    let s = SourceSpec::new("whoop", "strap");
    let (id, _) = begin(&c, Begin { source: &s, parser: "t", blob: None, uid: None, candidates: None }, NOW).unwrap();
    let mut rows: Vec<ObsRow> = (0..200)
        .map(|d| ObsRow {
            metric: RHR.into(),
            start_ms: NOW - d * DAY,
            end_ms: None,
            tz_min: 0,
            value: Value::Int(55),
            unit: None,
            detail: None,
        })
        .collect();
    rows.push(ObsRow { metric: GLU.into(), start_ms: NOW - DAY, end_ms: None, tz_min: 0, value: Value::Int(90), unit: None, detail: None });
    write(&c, id, &rows).unwrap();
    finish(&c, id, NOW).unwrap();
    c
}

fn cardio_90_days() -> GrantSpec {
    GrantSpec {
        name: "agent".into(),
        groups: vec!["cardio".into()],
        detail: false,
        window: Window::LastDays(90),
        reports: false,
        write: WriteLevel::None,
        expires_ms: NOW + DAY,
    }
}

#[test]
fn reads_stay_inside_the_window_and_groups() {
    let c = seeded();
    let s = Scope::of(&cardio_90_days(), NOW);
    let days = query::daily(&c, &s, RHR, None, None).unwrap();
    // only days wholly inside the window (any tz within ±14 h), so no pre-window point leaks into a sum
    assert!((88..=90).contains(&days.len()), "{}", days.len());
    assert!(days.iter().all(|d| health_core::daily::day_start(&d.day).unwrap() - 14 * 3_600_000 >= NOW - 90 * DAY));
    assert_eq!(query::daily(&c, &s, GLU, None, None).unwrap_err().code, "out_of_scope");
    let latest = query::latest(&c, &s, None).unwrap();
    assert_eq!(latest.iter().map(|p| p.metric.as_str()).collect::<Vec<_>>(), vec![RHR]);
}

#[test]
fn withheld_says_what_the_grant_hides() {
    let c = seeded();
    let w = query::withheld(&c, &Scope::of(&cardio_90_days(), NOW)).unwrap();
    assert_eq!((w.metrics, w.earlier_data), (1, true));
    let all = query::withheld(&c, &Scope::all()).unwrap();
    assert_eq!((all.metrics, all.earlier_data), (0, false));
}

#[test]
fn out_of_scope_and_unknown_codes_look_the_same() {
    let c = seeded();
    let s = Scope::of(&cardio_90_days(), NOW);
    for code in ["x:ocr/hiv-viral-load", "x:ocr/never-created", GLU] {
        assert_eq!(query::obs_query(&c, &s, code, None, None, None, 10).unwrap_err().code, "out_of_scope");
    }
    assert!(query::metric_search(&c, &s, "x:ocr", 10).unwrap().is_empty());
}

#[test]
fn delegates_see_only_safe_detail_fields() {
    let c = db::open_memory().unwrap();
    let s = SourceSpec::new("whoop", "user-4711");
    let (id, _) = begin(&c, Begin { source: &s, parser: "t", blob: None, uid: None, candidates: None }, NOW).unwrap();
    let detail = serde_json::json!({"comparator": "<", "ref": "50-100", "whoop_id": "rec-9", "raw": {"value": 1}, "s": "sensor.bedroom"});
    write(
        &c,
        id,
        &[ObsRow { metric: RHR.into(), start_ms: NOW, end_ms: None, tz_min: 0, value: Value::Int(50), unit: None, detail: Some(detail) }],
    )
    .unwrap();
    finish(&c, id, NOW).unwrap();
    let g = GrantSpec { detail: true, ..cardio_90_days() };
    let p = &query::obs_query(&c, &Scope::of(&g, NOW), RHR, None, None, None, 1).unwrap()[0];
    assert_eq!(p.detail, Some(serde_json::json!({"comparator": "<", "ref": "50-100"})));
    let owner = &query::obs_query(&c, &Scope::all(), RHR, None, None, None, 1).unwrap()[0];
    assert!(owner.detail.as_ref().unwrap().get("whoop_id").is_some());
}
