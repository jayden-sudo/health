//! Grants, tokens and audit through the AccessDb interface.

use health_core::access::{AccessDb, Audit, GrantSpec, Window, WriteLevel};

const NOW: i64 = 1_795_176_000_000;
const DAY: i64 = 86_400_000;

fn spec(name: &str) -> GrantSpec {
    GrantSpec {
        name: name.into(),
        groups: vec!["cardio".into()],
        detail: false,
        window: Window::LastDays(90),
        reports: false,
        write: WriteLevel::Derived,
        expires_ms: NOW + 30 * DAY,
        profile: false,
    }
}

#[test]
fn a_token_resolves_to_its_grant_until_revoked() {
    let a = AccessDb::open_memory().unwrap();
    let (g, token) = a.create(&spec("Claude"), NOW).unwrap();
    assert_eq!(a.resolve(&token, NOW).unwrap().id, g.id);
    assert_eq!(a.resolve("hk_wrong", NOW).unwrap_err().code, "denied");
    a.revoke(g.id, NOW).unwrap();
    assert_eq!(a.resolve(&token, NOW).unwrap_err().code, "denied");
}

#[test]
fn an_expired_grant_is_denied_and_tokens_are_never_listed() {
    let a = AccessDb::open_memory().unwrap();
    let (_, token) = a.create(&spec("Claude"), NOW).unwrap();
    assert_eq!(a.resolve(&token, NOW + 31 * DAY).unwrap_err().code, "denied");
    let listed = serde_json::to_string(&a.list().unwrap()).unwrap();
    assert!(!listed.contains(&token[3..]));
}

#[test]
fn audit_keeps_who_what_when_but_no_values() {
    let a = AccessDb::open_memory().unwrap();
    let (g, _) = a.create(&spec("Claude"), NOW).unwrap();
    a.record(&Audit {
        at: NOW,
        grant_id: g.id,
        op: "daily.query".into(),
        metrics: vec!["loinc:40443-4".into()],
        from_ms: Some(NOW - 90 * DAY),
        to_ms: Some(NOW),
        rows: 90,
        denied: false,
    })
    .unwrap();
    let log = a.audit(Some(g.id), 10).unwrap();
    assert_eq!((log.len(), log[0].rows, log[0].op.as_str()), (1, 90, "daily.query"));
}

#[test]
fn agent_rows_far_in_the_future_are_refused() {
    use health_core::derived::{Derive, MetricDef, write_derived};
    const NOW: i64 = 1_762_704_000_000;
    let c = health_core::db::open_memory().unwrap();
    let at = |t| health_core::write::ObsRow {
        metric: String::new(),
        start_ms: t,
        end_ms: None,
        tz_min: 0,
        value: health_core::Value::Int(1),
        unit: None,
        detail: None,
    };
    let d = Derive {
        algo: "a".into(),
        version: "1".into(),
        metric: MetricDef { name: "lvl".into(), unit: "".into(), agg: "level".into() },
        inputs: vec![],
        rows: vec![at(NOW), at(10_413_792_000_000)], // year 2300
        replace: true,
    };
    let grant = health_core::access::GrantSpec {
        name: "x".into(),
        groups: vec!["derived".into()],
        detail: true,
        window: health_core::access::Window::LastDays(30),
        reports: false,
        write: health_core::access::WriteLevel::Derived,
        expires_ms: NOW + 1,
        profile: false,
    };
    let scope = health_core::scope::Scope::of(&grant, NOW);
    let e = write_derived(&c, &scope, "g1", &d, NOW).unwrap_err();
    assert_eq!(e.code, "future");
    let n: i64 = c.query_row("SELECT count(*) FROM daily", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 0);
}
