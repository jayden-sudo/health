//! OCR in the data layer: the web's rules on the web's own outputs (fixtures from scripts/gen-ocr-data.ts),
//! and the review flow: invisible until confirmed, then rows, new measures and aliases.

use health_core::ocr::{self, Choice, OcrItem, Printed, Recognized, RecognizedItem};
use health_core::scope::Scope;
use health_core::{db, query, units};
use serde_json::Value as J;

fn fixtures() -> J {
    serde_json::from_str(include_str!("../../health-ocr/tests/fixtures/web-ocr.json")).unwrap()
}

fn printed(v: &J) -> Printed {
    match v {
        J::String(s) => Printed::Text(s.clone()),
        n => Printed::Num(n.as_f64().unwrap()),
    }
}

#[test]
fn direct_codes_match_the_web() {
    for case in fixtures()["codeFor"].as_array().unwrap() {
        let got = ocr::code_for(case["name"].as_str().unwrap(), case["panel"].as_str(), case["specimen"].as_str(), &printed(&case["v"]));
        assert_eq!(got.as_deref(), case["out"].as_str(), "{case}");
    }
}

#[test]
fn units_match_the_web() {
    let f = fixtures();
    for case in f["unitKey"].as_array().unwrap() {
        assert_eq!(units::unit_key(case["in"].as_str().unwrap()), case["out"].as_str().unwrap(), "{case}");
    }
    for case in f["pickUnit"].as_array().unwrap() {
        let us: Vec<String> = serde_json::from_value(case["units"].clone()).unwrap();
        assert_eq!(ocr::pick_unit(&us, case["printed"].as_str().unwrap()).as_deref(), case["out"].as_str(), "{case}");
    }
}

#[test]
fn new_measure_codes_match_the_web() {
    for case in fixtures()["newMetric"].as_array().unwrap() {
        let i = &case["in"];
        let m = ocr::new_metric_of(i["name"].as_str().unwrap(), i["unit"].as_str().unwrap(), &printed(&i["v"]));
        assert_eq!(serde_json::to_value(&m).unwrap(), case["out"], "{case}");
    }
}

const NOW: i64 = 1_795_176_000_000;
const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn item(name: &str, v: Printed, unit: &str) -> RecognizedItem {
    RecognizedItem {
        item: OcrItem {
            name: name.into(),
            panel: None,
            specimen: None,
            v,
            cmp: None,
            unit: unit.into(),
            reference: Some("3.9-6.1".into()),
            flag: Some("H".into()),
            text: format!("{name}  line"),
            code: None,
            suggest: vec![],
        },
        pre: None,
    }
}

fn report() -> Recognized {
    Recognized {
        items: vec![
            item("葡萄糖", Printed::Num(6.8), "mmol/L"),
            item("Ferritin-like X", Printed::Num(12.0), "ng/mL"),
            item("尿胆原", Printed::Text("Negative".into()), ""),
            item("Mystery", Printed::Num(3.0), ""),
        ],
        text: "示例 检验报告单\n葡萄糖 6.8".into(),
        date: Some("2026-10-12".into()),
    }
}

#[test]
fn a_report_is_invisible_until_confirmed_then_rows_measures_and_aliases() {
    let c = db::open_memory().unwrap();
    let r = ocr::ingest(&c, SHA, &report(), NOW - 86_400_000, 480, NOW).unwrap();
    assert_eq!(r.items, 4);
    let cand = ocr::candidates(&c, r.import_id).unwrap();
    // glucose is preselected by the common-items table
    assert!(cand.items[0].code.is_some(), "{:?}", cand.items[0]);
    let all = Scope::all();
    assert!(query::latest(&c, &all, None).unwrap().is_empty(), "review rows must be invisible");

    // two items on one measure are refused
    let glu = cand.items[0].code.clone().unwrap();
    let dup = vec![
        Choice { code: Some(glu.clone()), unit: Some("mmol/L".into()), ..Default::default() },
        Choice { code: Some(glu.clone()), unit: Some("mmol/L".into()), ..Default::default() },
        Choice { skip: true, ..Default::default() },
        Choice { skip: true, ..Default::default() },
    ];
    assert_eq!(ocr::confirm(&c, r.import_id, &dup, NOW, 480, NOW).unwrap_err().code, "duplicate_target");

    let choices = vec![
        Choice { code: Some(glu.clone()), unit: Some("mmol/L".into()), ..Default::default() },
        Choice { new: true, ..Default::default() },
        Choice { new: true, ..Default::default() },
        Choice::default(), // nothing chosen: left out, not an error
    ];
    let at = NOW - 3 * 86_400_000;
    let out = ocr::confirm(&c, r.import_id, &choices, at, 480, NOW).unwrap();
    assert_eq!((out.saved, out.left, out.created), (3, 1, 2));
    let latest = query::latest(&c, &all, None).unwrap();
    let g = latest.iter().find(|p| p.metric == glu).unwrap();
    assert_eq!(g.start_ms, at);
    assert_eq!(g.detail.as_ref().unwrap()["printed"]["flag"], "H");
    assert!(latest.iter().any(|p| p.metric == "x:ocr/ferritin-like-x.ng-ml"));
    assert!(latest.iter().any(|p| p.metric.starts_with("x:ocr/尿胆原")));
    // learnt: the same name and unit map to the same measure next time
    assert_eq!(
        health_core::review::lookup_alias(&c, "Ferritin-like X", "ng/mL", None).unwrap().as_deref(),
        Some("x:ocr/ferritin-like-x.ng-ml")
    );
    // the report text is searchable now
    assert!(!query::report_search(&c, &all, "检验报告单", 5).unwrap().is_empty());
}

#[test]
fn the_same_file_is_refused_twice() {
    let c = db::open_memory().unwrap();
    ocr::ingest(&c, SHA, &report(), NOW, 480, NOW).unwrap();
    assert_eq!(ocr::ingest(&c, SHA, &report(), NOW, 480, NOW).unwrap_err().code, "blob_dup");
}

#[test]
fn a_text_result_cannot_go_on_a_numeric_measure() {
    let c = db::open_memory().unwrap();
    let r = ocr::ingest(&c, SHA, &report(), NOW, 480, NOW).unwrap();
    // "Negative" on glucose: not ready, so left out; with nothing else chosen there's nothing to save
    let mut choices = vec![Choice { skip: true, ..Default::default() }; 4];
    choices[2] = Choice { code: Some("loinc:2345-7".into()), unit: Some("mmol/L".into()), ..Default::default() };
    assert_eq!(ocr::confirm(&c, r.import_id, &choices, NOW, 480, NOW).unwrap_err().code, "nothing_to_save");
}

#[test]
fn a_second_report_of_the_same_day_does_not_replace_the_first() {
    let c = db::open_memory().unwrap();
    let one = |sha: &str, v: f64| {
        let mut r = report();
        r.items = vec![item("葡萄糖", Printed::Num(v), "mmol/L")];
        ocr::ingest(&c, sha, &r, NOW, 480, NOW).unwrap().import_id
    };
    let pick = vec![Choice { code: Some("loinc:2345-7".into()), ..Default::default() }];
    let a = one(SHA, 5.5);
    ocr::confirm(&c, a, &pick, NOW, 480, NOW).unwrap();
    let b = one(&"b".repeat(64), 9.9);
    assert_eq!(ocr::confirm(&c, b, &pick, NOW, 480, NOW).unwrap_err().code, "same_day_report");
    let v = query::latest(&c, &Scope::all(), Some("loinc:2345-7")).unwrap();
    // stored in glucose's canonical mg/dL: 5.5 mmol/L ≈ 99.09, not 9.9 mmol/L ≈ 178
    let health_core::Value::Real(x) = v[0].value else { panic!("{:?}", v[0].value) };
    assert!((x - 99.09).abs() < 0.01, "the first report's value stays: {x}");
    // a different day is fine
    ocr::confirm(&c, b, &pick, NOW + 86_400_000, 480, NOW).unwrap();
}

#[test]
fn a_new_measure_keeps_the_unit_spelling_it_was_created_with() {
    let c = db::open_memory().unwrap();
    let with_unit = |sha: &str, unit: &str, at: i64| {
        let mut r = report();
        r.items = vec![item("Zorb", Printed::Num(1.0), unit)];
        let id = ocr::ingest(&c, sha, &r, at, 480, NOW).unwrap().import_id;
        ocr::confirm(&c, id, &[Choice { new: true, ..Default::default() }], at, 480, NOW)
    };
    with_unit(SHA, "mg/dL", NOW).unwrap();
    with_unit(&"c".repeat(64), "mg/dl", NOW + 86_400_000).unwrap();
}
