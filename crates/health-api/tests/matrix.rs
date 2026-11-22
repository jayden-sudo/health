//! Permission matrix (plan §5.4): every op × kinds of caller, through the real call path.
//! A denial must return `denied`, leave the database unchanged, and be audited.

use health_api::{Caller, Ctx, Delegate, call, ops};
use health_core::access::{GrantSpec, Window, WriteLevel};
use health_core::keys::MemoryStore;
use health_core::profile::Profile;
use serde_json::{Value, json};

const NOW: i64 = 1_795_176_000_000;
const DAY: i64 = 86_400_000;
const RHR: &str = "loinc:40443-4";

fn setup() -> (tempfile::TempDir, Profile) {
    let d = tempfile::tempdir().unwrap();
    let p = Profile::init(d.path(), "default", &MemoryStore::default()).unwrap();
    let mut owner = Ctx::new(&p, NOW, Caller::Owner);
    let rows: Vec<Value> = (0..120).map(|i| json!({"metric": RHR, "start_ms": NOW - i * DAY, "tz_min": 0, "value": 50 + i % 7})).collect();
    let r = call(
        &mut owner,
        "derived.write",
        json!({"algo": "seed", "version": "1", "metric": {"name": "x", "unit": "", "agg": "instant"}, "inputs": [], "rows": [{"metric": "", "start_ms": NOW, "tz_min": 0, "value": 1}]}),
    );
    assert!(r.is_ok());
    // measured data: resting HR (cardio) and glucose (lab) through the normal pipeline
    let c = &p.db;
    let s = health_core::write::SourceSpec::new("whoop", "user-4711");
    let (id, _) =
        health_core::write::begin(c, health_core::write::Begin { source: &s, parser: "t", blob: None, uid: None, candidates: None }, NOW)
            .unwrap();
    let mut obs: Vec<health_core::write::ObsRow> = rows.into_iter().map(|v| serde_json::from_value(v).unwrap()).collect();
    obs.push(serde_json::from_value(json!({"metric": "loinc:2345-7", "start_ms": NOW - DAY, "tz_min": 0, "value": 90})).unwrap());
    health_core::write::write(c, id, &obs).unwrap();
    health_core::write::finish(c, id, NOW).unwrap();
    (d, p)
}

fn grant(p: &Profile, detail: bool, write: WriteLevel, groups: &[&str]) -> Caller {
    let spec = GrantSpec {
        name: "agent".into(),
        groups: groups.iter().map(|g| g.to_string()).collect(),
        detail,
        window: Window::LastDays(90),
        reports: false,
        write,
        expires_ms: NOW + DAY,
    };
    let (_, token) = p.access.create(&spec, NOW).unwrap();
    Caller::Delegate(p.access.resolve(&token, NOW).unwrap())
}

/// Everything a delegate could change, as one comparable value.
fn snapshot(p: &Profile) -> (i64, i64, i64, i64, i64) {
    let n = |sql: &str| p.db.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap();
    (
        n("SELECT count(*) FROM observation"),
        n("SELECT count(*) FROM import"),
        n("SELECT count(*) FROM correction"),
        n("SELECT count(*) FROM source"),
        n("SELECT coalesce(sum(priority), 0) FROM source"),
    )
}

fn code(r: Result<health_api::Outcome, health_core::Error>) -> &'static str {
    match r {
        Ok(_) => "ok",
        Err(e) => e.code,
    }
}

#[test]
fn delegates_reach_exactly_the_listed_ops() {
    let open: Vec<&str> = ops().iter().filter(|o| o.delegate != Delegate::Never).map(|o| o.name).collect();
    // A new op is Never until it is deliberately added here.
    assert_eq!(
        open,
        [
            "metric.search",
            "metric.show",
            "obs.query",
            "obs.latest",
            "daily.query",
            "source.list",
            "report.search",
            "report.show",
            "whoami",
            "review.propose",
            "derived.write"
        ]
    );
}

#[test]
fn never_ops_are_denied_unchanged_and_audited_for_every_grant() {
    let (_d, p) = setup();
    let callers = [
        grant(&p, false, WriteLevel::None, &["cardio"]),
        grant(&p, true, WriteLevel::Propose, &["cardio", "lab"]),
        grant(
            &p,
            true,
            WriteLevel::Derived,
            &["body", "sleep", "cardio", "activity", "lab", "infectious", "tumor", "repro", "custom", "lab_other", "other", "derived"],
        ),
    ];
    for caller in callers {
        for op in ops().iter().filter(|o| o.delegate == Delegate::Never) {
            let before = snapshot(&p);
            let audited = p.access.audit(None, 1_000_000).unwrap().len();
            let mut ctx = Ctx::new(&p, NOW, caller.clone());
            assert_eq!(code(call(&mut ctx, op.name, json!({}))), "denied", "{}", op.name);
            assert_eq!(snapshot(&p), before, "{}", op.name);
            let log = p.access.audit(None, 1).unwrap();
            assert_eq!((p.access.audit(None, 1_000_000).unwrap().len(), log[0].denied), (audited + 1, true), "{}", op.name);
        }
    }
}

#[test]
fn a_daily_cardio_grant_reads_its_trend_and_nothing_else() {
    let (_d, p) = setup();
    let mut ctx = Ctx::new(&p, NOW, grant(&p, false, WriteLevel::None, &["cardio"]));
    let out = call(&mut ctx, "daily.query", json!({"metric": RHR})).unwrap();
    let days = out.value.as_array().unwrap();
    assert!((88..=90).contains(&days.len()));
    let w = out.withheld.unwrap();
    assert!(w.metrics >= 2 && w.earlier_data); // glucose and the derived seed are hidden; older RHR exists
    assert_eq!(code(call(&mut ctx, "obs.query", json!({"metric": RHR}))), "denied"); // daily only
    assert_eq!(code(call(&mut ctx, "daily.query", json!({"metric": "loinc:2345-7"}))), "out_of_scope");
    assert_eq!(code(call(&mut ctx, "report.search", json!({"q": "abc"}))), "denied");
    assert_eq!(code(call(&mut ctx, "review.propose", json!({"rows": []}))), "denied");
    let src = call(&mut ctx, "source.list", json!({})).unwrap().value;
    assert!(!src.to_string().contains("user-4711"), "{src}");
}

#[test]
fn proposals_stay_invisible_until_the_owner_confirms() {
    let (_d, p) = setup();
    let mut agent = Ctx::new(&p, NOW, grant(&p, true, WriteLevel::Propose, &["cardio"]));
    let row = json!({"metric": RHR, "start_ms": NOW + 1, "tz_min": 0, "value": 48});
    assert_eq!(
        code(call(&mut agent, "review.propose", json!({"rows": [{"metric": "loinc:2345-7", "start_ms": NOW, "tz_min": 0, "value": 1}]}))),
        "out_of_scope"
    );
    let id =
        call(&mut agent, "review.propose", json!({"rows": [row], "note": "from my notes"})).unwrap().value["import_id"].as_i64().unwrap();
    let latest = |ctx: &mut Ctx| call(ctx, "obs.latest", json!({"metric": RHR})).unwrap().value[0]["value"].clone();
    assert_ne!(latest(&mut agent), json!(48));
    assert_eq!(code(call(&mut agent, "derived.write", json!({}))), "denied");
    let mut owner = Ctx::new(&p, NOW, Caller::Owner);
    call(&mut owner, "review.confirm", json!({"id": id})).unwrap();
    assert_eq!(latest(&mut owner), json!(48));
}

#[test]
fn derived_results_get_their_own_metric_and_never_win_the_election() {
    let (_d, p) = setup();
    let mut agent = Ctx::new(&p, NOW, grant(&p, false, WriteLevel::Derived, &["cardio", "derived"]));
    let req = json!({
        "algo": "rhr-trend", "version": "1", "metric": {"name": "slope", "unit": "/min/d", "agg": "instant"},
        "inputs": [{"metric": RHR, "from_ms": NOW - 90 * DAY, "to_ms": NOW}],
        "rows": [{"metric": "ignored", "start_ms": NOW, "tz_min": 0, "value": -0.02}]
    });
    let out = call(&mut agent, "derived.write", req.clone()).unwrap().value;
    // a daily-only grant isn't told how many readings fell in its range (narrowing it would reveal their times)
    assert_eq!((out["metric"].as_str(), out.get("inputs")), (Some("x:derived/rhr-trend/slope"), None));
    // a second run replaces the first
    assert_eq!(call(&mut agent, "derived.write", req).unwrap().value["replaced"], 1);
    let d = call(&mut agent, "daily.query", json!({"metric": "x:derived/rhr-trend/slope"})).unwrap().value;
    assert_eq!(d.as_array().unwrap().len(), 1);
    let prio: i64 =
        p.db.query_row("SELECT priority FROM source WHERE connector = 'derived' AND external_id LIKE 'rhr-trend/g%'", [], |r| r.get(0))
            .unwrap();
    assert_eq!(prio, health_core::derived::PRIORITY);
    // a second output of the same algorithm is kept; a new version replaces the old run
    let other = json!({"algo": "rhr-trend", "version": "1", "metric": {"name": "intercept", "unit": "/min", "agg": "instant"}, "inputs": [], "rows": [{"metric": "x", "start_ms": NOW, "tz_min": 0, "value": 55}]});
    assert_eq!(call(&mut agent, "derived.write", other).unwrap().value["replaced"], 0);
    let mut v2 = json!({"algo": "rhr-trend", "version": "2", "metric": {"name": "slope", "unit": "/min/d", "agg": "instant"}, "inputs": [], "rows": [{"metric": "x", "start_ms": NOW, "tz_min": 0, "value": -0.03}]});
    assert_eq!(call(&mut agent, "derived.write", v2.take()).unwrap().value["replaced"], 1);
    let latest =
        |code: &str| call(&mut Ctx::new(&p, NOW, Caller::Owner), "obs.latest", json!({"metric": code})).unwrap().value[0]["value"].clone();
    assert_eq!((latest("x:derived/rhr-trend/slope"), latest("x:derived/rhr-trend/intercept")), (json!(-0.03), json!(55)));
    // reads were audited with their metrics, never values
    let log = p.access.audit(None, 10).unwrap();
    assert!(log.iter().any(|a| a.op == "derived.write" && a.metrics.contains(&RHR.to_string())));
    assert!(log.iter().any(|a| a.op == "derived.write" && a.metrics.contains(&"x:derived/rhr-trend/slope".to_string())));
}

#[test]
fn a_detailed_grant_is_told_its_input_count_under_a_writer_key_that_survives_new_grant_ids() {
    let (_d, p) = setup();
    let caller = grant(&p, true, WriteLevel::Derived, &["cardio", "derived"]);
    let Caller::Delegate(g) = &caller else { unreachable!() };
    assert!(g.writer.starts_with('g') && g.writer.len() == 17, "{}", g.writer);
    let mut agent = Ctx::new(&p, NOW, caller.clone());
    let req = json!({
        "algo": "rhr-trend", "version": "1", "metric": {"name": "slope", "unit": "", "agg": "instant"},
        "inputs": [{"metric": RHR, "from_ms": NOW - 90 * DAY, "to_ms": NOW}],
        "rows": [{"metric": "x", "start_ms": NOW, "tz_min": 0, "value": 1}]
    });
    assert_eq!(call(&mut agent, "derived.write", req).unwrap().value["inputs"], 1);
}

#[test]
fn a_dry_run_never_creates_or_revokes_a_grant_or_writes_a_file() {
    let (d, p) = setup();
    let mut owner = Ctx::new(&p, NOW, Caller::Owner);
    owner.dry_run = true;
    let spec = json!({"name": "x", "groups": ["cardio"], "detail": false, "window": "all", "reports": false, "write": "none", "expires_ms": NOW + DAY});
    assert_eq!(code(call(&mut owner, "grant.create", spec)), "no_dry_run");
    assert!(p.access.list().unwrap().is_empty());
    let out = d.path().join("x.csv");
    assert_eq!(code(call(&mut owner, "export.csv", json!({"path": out, "from_ms": 0}))), "no_dry_run");
    assert!(!out.exists());
}

#[test]
fn revoked_and_expired_tokens_resolve_to_nothing() {
    let (_d, p) = setup();
    let spec = GrantSpec {
        name: "a".into(),
        groups: vec!["cardio".into()],
        detail: true,
        window: Window::All,
        reports: false,
        write: WriteLevel::None,
        expires_ms: NOW + DAY,
    };
    let (g, t) = p.access.create(&spec, NOW).unwrap();
    assert_eq!(p.access.resolve(&t, NOW + 2 * DAY).unwrap_err().code, "denied");
    p.access.revoke(g.id, NOW).unwrap();
    assert_eq!(p.access.resolve(&t, NOW).unwrap_err().code, "denied");
}

#[test]
fn delegate_request_schemas_are_self_contained_objects() {
    for op in ops().iter().filter(|o| o.delegate != Delegate::Never) {
        let s = serde_json::to_value((op.req)()).unwrap();
        assert_eq!(s["type"], "object", "{}", op.name);
        let text = s.to_string();
        assert!(!text.contains("$ref") && !text.contains("$schema"), "{}: {text}", op.name);
    }
}

#[test]
fn a_failed_delegate_write_is_audited_as_refused() {
    let (_d, p) = setup();
    let mut agent = Ctx::new(&p, NOW, grant(&p, true, WriteLevel::Propose, &["cardio"]));
    let out_of_scope = json!({"rows": [{"metric": "loinc:2345-7", "start_ms": NOW, "tz_min": 0, "value": 1}]});
    assert_eq!(code(call(&mut agent, "review.propose", out_of_scope)), "out_of_scope");
    let last = &p.access.audit(None, 1).unwrap()[0];
    assert_eq!((last.op.as_str(), last.denied, last.rows), ("review.propose", true, 0));
}

#[test]
fn delegate_writes_stay_inside_the_grant_window() {
    let (_d, p) = setup();
    let mut agent = Ctx::new(&p, NOW, grant(&p, true, WriteLevel::Derived, &["cardio", "derived"]));
    let old = json!({"rows": [{"metric": RHR, "start_ms": NOW - 400 * DAY, "tz_min": 0, "value": 50}]});
    assert_eq!(code(call(&mut agent, "review.propose", old)), "out_of_scope");
    let derived = json!({"algo": "a", "version": "1", "metric": {"name": "b", "unit": "", "agg": "instant"}, "inputs": [],
        "rows": [{"metric": "x", "start_ms": NOW - 400 * DAY, "tz_min": 0, "value": 1}]});
    assert_eq!(code(call(&mut agent, "derived.write", derived)), "out_of_scope");
}

#[test]
fn the_owner_corrects_a_confirmed_lab_photo_value() {
    let (_d, p) = setup();
    let c = &p.db;
    let s = health_core::write::SourceSpec::new("ocr", "photo");
    let (id, _) = health_core::write::begin(
        c,
        health_core::write::Begin { source: &s, parser: "ocr@1", blob: None, uid: None, candidates: Some(json!({})) },
        NOW,
    )
    .unwrap();
    let row: health_core::write::ObsRow =
        serde_json::from_value(json!({"metric": "loinc:2345-7", "start_ms": NOW, "tz_min": 0, "value": 900})).unwrap();
    health_core::review::confirm(c, id, &[row], None, &[], NOW).unwrap();
    let sid: i64 = c.query_row("SELECT id FROM source WHERE connector = 'ocr'", [], |r| r.get(0)).unwrap();
    let mut owner = Ctx::new(&p, NOW, Caller::Owner);
    let r = call(&mut owner, "correct", json!({"metric": "loinc:2345-7", "source_id": sid, "start_ms": NOW, "value": 90})).unwrap();
    assert_eq!(r.value["applied"], true);
    let v = call(&mut owner, "obs.latest", json!({"metric": "loinc:2345-7"})).unwrap().value;
    assert_eq!((v[0]["value"].clone(), v[0]["corrected"].clone()), (json!(90), json!(true)));
}
