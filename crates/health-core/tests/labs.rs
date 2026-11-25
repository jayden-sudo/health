//! Lab device results read as measurements (labs.rs): ForceDecks jumps grouped into times on the plates with the
//! best jump, seca body composition measurements, through the database and its corrections.

use health_core::correction::{self, Key};
use health_core::keys::MemoryStore;
use health_core::labs;
use health_core::profile::Profile;
use health_core::write::{Begin, ObsRow, SourceSpec, begin, finish, write};
use health_core::{Value, query};

const T0: i64 = 1_795_000_000_000;

fn result(p: &Profile, device: &str, at: i64, rows: &[(&str, Value)]) {
    let src = SourceSpec::new("lab", device);
    let (id, _) = begin(&p.db, Begin { source: &src, parser: "result", blob: None, uid: None, candidates: None }, T0).unwrap();
    let rows: Vec<ObsRow> = rows
        .iter()
        .map(|(m, v)| ObsRow { metric: (*m).into(), start_ms: at, end_ms: None, tz_min: 480, value: v.clone(), unit: None, detail: None })
        .collect();
    write(&p.db, id, &rows).unwrap();
    finish(&p.db, id, T0).unwrap();
}

#[test]
fn forcedecks_jumps_become_sessions_with_their_best_and_seca_values_one_measurement() {
    let d = tempfile::tempdir().unwrap();
    let p = Profile::init(d.path(), "a", &MemoryStore::default()).unwrap();
    let h = "x:fd/cmj/jump_height_imp_mom";
    result(&p, "forcedecks-01", T0 - 600_000, &[(h, Value::Real(31.2)), ("x:fd/cmj/peak_power", Value::Int(3100))]);
    result(&p, "forcedecks-01", T0 - 480_000, &[(h, Value::Real(33.8)), ("x:fd/cmj/peak_power", Value::Int(3250))]);
    result(&p, "seca-mbca-01", T0 - 86_400_000, &[("x:seca/smm_over_age", Value::Real(31.4)), ("loinc:29463-7", Value::Real(70.2))]);
    let s = labs::fd_sessions(&p.db).unwrap();
    assert_eq!(s.len(), 1);
    assert_eq!((s[0].device.as_str(), s[0].test.as_str(), s[0].trials.len(), s[0].best), ("forcedecks-01", "cmj", 2, 1));
    // a correction counts: the second jump corrected down, the first is best now
    let (m, src): (i64, i64) = p
        .db
        .query_row("SELECT o.metric_id, o.source_id FROM observation o JOIN metric m ON m.id = o.metric_id WHERE m.code = ? ORDER BY o.start_ms DESC LIMIT 1", [h], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap();
    let at = correction::next_at(&p.db, T0).unwrap();
    correction::put(&p.db, Key { metric_id: m, source_id: src, start_ms: T0 - 480_000 }, at, &Value::Real(30.0), T0).unwrap();
    assert_eq!(labs::fd_sessions(&p.db).unwrap()[0].best, 0);
    // seca: the whole measurement, LOINC weight included
    let scans = labs::body_scans(&p.db).unwrap();
    assert_eq!(scans.len(), 1);
    assert_eq!(scans[0].values.len(), 2);
    // the home page shows ForceDecks under its own card; exports keep it under activity
    let latest = query::latest(&p.db, &health_core::scope::Scope::all(), None).unwrap();
    let fd = latest.iter().find(|x| x.metric == h).unwrap();
    assert_eq!((fd.category.as_str(), fd.group.as_str()), ("force", "activity"));
}
