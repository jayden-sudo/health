//! The personal profile: the owner reads and writes it; a delegate sees age and sex only, and only when its
//! grant says so (owner, 2026-11-22).

use health_api::{Caller, Ctx, call};
use health_core::access::{GrantSpec, Window, WriteLevel};
use health_core::keys::MemoryStore;
use health_core::profile::Profile;
use serde_json::json;

const NOW: i64 = 1_795_176_000_000; // 2026-11-21

fn grant(p: &Profile, profile: bool) -> health_core::access::Grant {
    let spec = GrantSpec {
        name: "agent".into(),
        groups: vec!["cardio".into()],
        detail: false,
        window: Window::All,
        reports: false,
        write: WriteLevel::None,
        expires_ms: NOW + 86_400_000,
        profile,
    };
    let (_, token) = p.access.create(&spec, NOW).unwrap();
    p.access.resolve(&token, NOW).unwrap()
}

#[test]
fn the_owner_sets_the_profile_and_agents_see_age_and_sex_only_when_granted() {
    let d = tempfile::tempdir().unwrap();
    let p = Profile::init(d.path(), "default", &MemoryStore::default()).unwrap();
    let mut owner = Ctx::new(&p, NOW, Caller::Owner);
    assert_eq!(call(&mut owner, "profile.get", json!({})).unwrap().value, json!({}));
    let v = call(&mut owner, "profile.set", json!({ "birth": "1990-12", "sex": "Female" })).unwrap().value;
    // November 2026: the December birthday hasn't come yet
    assert_eq!(v, json!({ "birth": "1990-12", "sex": "Female", "age": 35 }));
    for bad in [json!({ "birth": "1990-13" }), json!({ "birth": "2027-01" }), json!({ "birth": "90-12" }), json!({ "sex": "x" })] {
        assert!(call(&mut owner, "profile.set", bad.clone()).is_err(), "{bad}");
    }
    // absent keeps, null clears
    let v = call(&mut owner, "profile.set", json!({ "sex": null })).unwrap().value;
    assert_eq!(v, json!({ "birth": "1990-12", "age": 35 }));
    call(&mut owner, "profile.set", json!({ "sex": "Female" })).unwrap();

    let mut without = Ctx::new(&p, NOW, Caller::Delegate(grant(&p, false)));
    assert_eq!(call(&mut without, "profile.get", json!({})).err().unwrap().code, "denied");
    let mut with = Ctx::new(&p, NOW, Caller::Delegate(grant(&p, true)));
    assert_eq!(call(&mut with, "profile.get", json!({})).unwrap().value, json!({ "sex": "Female", "age": 35 }));
    // never writable by an agent; both calls are in the access log, the refused one as refused
    assert!(call(&mut with, "profile.set", json!({ "sex": "Male" })).is_err());
    let log = p.access.audit(None, 10).unwrap();
    assert!(log.iter().any(|a| a.op == "profile.get" && a.denied));
    assert!(log.iter().any(|a| a.op == "profile.get" && !a.denied));
}
