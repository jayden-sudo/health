//! A checked result file stored (protocol 2.3 steps 4–5): once, as one batch; anything uncertain goes to review.

use health_core::result::{Outcome, ingest};
use health_core::scope::Scope;
use health_core::{db, query};
use serde_json::{Map, Value as J, json};

fn doc(v: J) -> Map<String, J> {
    v.as_object().unwrap().clone()
}
const NOW: i64 = 1_795_176_000_000;

fn lab(id: &str, obs: J) -> Map<String, J> {
    doc(json!({ "v": 1, "id": id, "uid": "a", "src": "lab:seca-mbca-01", "at": 1_795_000_000_000i64, "tz": 480, "obs": obs, "sig": {} }))
}

#[test]
fn a_result_is_stored_once_with_its_id_as_the_batch_uid() {
    let c = db::open_memory().unwrap();
    let d = lab("0193a5c2-7e1f-7b3a-9c4d-2f6e8a1b5c90", json!([{ "code": "loinc:29463-7", "v": 68.4, "u": "kg" }]));
    let r = ingest(&c, &d, NOW).unwrap();
    assert_eq!((r.outcome, r.written), (Outcome::Saved, 1));
    let uid: String = c.query_row("SELECT uid FROM import WHERE id = ?", [r.import_id.unwrap()], |r| r.get(0)).unwrap();
    assert_eq!(uid, "0193a5c2-7e1f-7b3a-9c4d-2f6e8a1b5c90");
    let (conn, ext): (String, String) = c
        .query_row(
            "SELECT connector, external_id FROM source s JOIN import i ON i.source_id = s.id WHERE i.id = ?",
            [r.import_id.unwrap()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!((conn.as_str(), ext.as_str()), ("lab", "seca-mbca-01"));
    let pts = query::latest(&c, &Scope::all(), Some("loinc:29463-7")).unwrap();
    assert_eq!(pts.len(), 1);
    // the same result again (the cursor wasn't advanced, or the backend sent it twice): nothing new
    assert_eq!(ingest(&c, &d, NOW).unwrap().outcome, Outcome::Seen);
}

#[test]
fn unknown_codes_units_and_refused_lines_put_the_whole_result_in_review() {
    let c = db::open_memory().unwrap();
    for (i, obs) in [
        json!([{ "code": "loinc:29463-7", "v": 68.4, "u": "kg" }, { "code": "x:no_such_measure", "v": 1 }]),
        json!([{ "code": "loinc:29463-7", "v": 68.4, "u": "furlong" }]),
        json!([{ "code": "loinc:29463-7", "v": "<5", "u": "kg" }]),
        // glucose has several units: none printed is never assumed
        json!([{ "code": "loinc:2345-7", "v": 5.5 }]),
    ]
    .into_iter()
    .enumerate()
    {
        let r = ingest(&c, &lab(&format!("0193a5c2-7e1f-7b3a-9c4d-2f6e8a1b5c9{i}"), obs), NOW).unwrap();
        assert_eq!((r.outcome, r.written), (Outcome::Review, 0), "case {i}");
        let meta: String = c.query_row("SELECT meta FROM import WHERE id = ?", [r.import_id.unwrap()], |r| r.get(0)).unwrap();
        let meta: J = serde_json::from_str(&meta).unwrap();
        assert_eq!(meta["candidates"]["kind"], "result");
        assert_eq!(meta["src"], "lab:seca-mbca-01");
    }
    // nothing of them is visible
    assert!(query::latest(&c, &Scope::all(), None).unwrap().is_empty());
}

#[test]
fn a_malformed_file_is_refused_as_bad_result() {
    let c = db::open_memory().unwrap();
    for bad in [
        json!({ "id": "x", "src": "lab:Seca", "at": 1, "obs": [] }),
        json!({ "id": "x", "src": "gw:seca", "at": 1, "obs": [] }),
        json!({ "id": "x", "src": "lab:seca", "at": -1, "obs": [] }),
        json!({ "id": "x", "src": "lab:seca", "at": 1, "tz": 900, "obs": [] }),
        json!({ "id": "x", "src": "lab:seca", "at": 1, "tz": 1.5, "obs": [] }),
        json!({ "id": "", "src": "lab:seca", "at": 1, "obs": [] }),
    ] {
        let e = ingest(&c, &doc(bad.clone()), NOW).unwrap_err();
        assert_eq!(e.code, "bad_result", "{bad}");
    }
    // 480.0 is a whole number, as JS sees it
    let d = doc(
        json!({ "id": "y", "src": "lab:seca", "at": 1_795_000_000_000i64, "tz": 480.0, "obs": [{ "code": "loinc:29463-7", "v": 70, "u": "kg" }] }),
    );
    assert_eq!(ingest(&c, &d, NOW).unwrap().outcome, Outcome::Saved);
}

#[test]
fn a_result_in_review_is_confirmed_line_by_line_with_the_web_rules() {
    use health_core::result::{LineChoice, confirm};
    let c = db::open_memory().unwrap();
    let d = lab(
        "0193a5c2-7e1f-7b3a-9c4d-2f6e8a1b5c90",
        json!([{ "code": "WT", "v": 68.4, "u": "kg" }, { "code": "GLU", "v": 5.5, "u": "mmol/L" }, { "code": "NOTE", "v": "fasting" }]),
    );
    let id = ingest(&c, &d, NOW).unwrap().import_id.unwrap();
    let ch = |code: Option<&str>, unit: Option<&str>, skip: bool| LineChoice {
        code: code.map(String::from),
        unit: unit.map(String::from),
        skip,
    };
    // every line settled; a unit for measures with units; never two lines on one measure at one time
    let e = confirm(&c, id, &[ch(Some("loinc:29463-7"), Some("kg"), false), ch(None, None, false), ch(None, None, true)], NOW).unwrap_err();
    assert_eq!(e.code, "line_open");
    let e = confirm(&c, id, &[ch(Some("loinc:29463-7"), None, false), ch(None, None, true), ch(None, None, true)], NOW).unwrap_err();
    assert_eq!(e.code, "unit_required");
    let e = confirm(
        &c,
        id,
        &[ch(Some("loinc:29463-7"), Some("kg"), false), ch(Some("loinc:29463-7"), Some("kg"), false), ch(None, None, true)],
        NOW,
    )
    .unwrap_err();
    assert_eq!(e.code, "duplicate_target");
    let e = confirm(&c, id, &[ch(None, None, true), ch(None, None, true), ch(None, None, true)], NOW).unwrap_err();
    assert_eq!(e.code, "nothing_to_save");
    assert!(query::latest(&c, &Scope::all(), None).unwrap().is_empty());
    let r = confirm(
        &c,
        id,
        &[ch(Some("loinc:29463-7"), Some("kg"), false), ch(Some("loinc:2345-7"), Some("mmol/L"), false), ch(None, None, true)],
        NOW,
    )
    .unwrap();
    assert_eq!(r.written, 2);
    let pts = query::latest(&c, &Scope::all(), None).unwrap();
    assert_eq!(pts.len(), 2);
}
