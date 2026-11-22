//! The pure OCR rules against the web's own outputs (fixtures from scripts/gen-ocr-data.ts): each test page's
//! lines (as the web engine read them) give the same table items and date, and the helpers agree case by case.

use health_ocr::types::Line;
use health_ocr::{date, det, rows};
use serde_json::{Value as J, json};

fn fixtures() -> J {
    serde_json::from_str(include_str!("fixtures/web-ocr.json")).unwrap()
}

fn page_lines(name: &str) -> Vec<Line> {
    let text = std::fs::read_to_string(format!("{}/tests/fixtures/{name}.web.json", env!("CARGO_MANIFEST_DIR"))).unwrap();
    serde_json::from_str(&text).unwrap()
}

#[test]
fn every_page_reads_as_the_same_table_and_date() {
    let f = fixtures();
    for (page, want) in f["tables"].as_object().unwrap() {
        let lines = page_lines(page);
        let t = rows::read_table(&lines);
        assert_eq!(t.header, want["header"].as_bool().unwrap(), "{page} header");
        assert!((rows::skew_of(&lines) - want["skew"].as_f64().unwrap()).abs() < 1e-12, "{page} skew");
        let got: Vec<J> = t.items.iter().map(|i| serde_json::to_value(i).unwrap()).collect();
        let want_items = want["items"].as_array().unwrap();
        for (k, (g, w)) in got.iter().zip(want_items).enumerate() {
            assert_eq!(g, w, "{page} item {k}");
        }
        assert_eq!(got.len(), want_items.len(), "{page} item count");
        let d = date::report_date(&lines, 2026).map(|d| json!({ "y": d.y, "m": d.m, "d": d.d }));
        assert_eq!(d.unwrap_or(J::Null), want["date"], "{page} date");
    }
}

#[test]
fn value_reference_and_unit_cells_parse_as_the_web() {
    let f = fixtures();
    for c in f["parseValue"].as_array().unwrap() {
        let got = rows::parse_value(c["in"].as_str().unwrap(), c["col"].as_bool().unwrap()).map(|p| {
            let mut o = json!({ "v": p.v });
            if let Some(x) = p.cmp {
                o["cmp"] = json!(x);
            }
            if let Some(x) = p.flag {
                o["flag"] = json!(x);
            }
            o
        });
        assert_eq!(got.unwrap_or(J::Null), c["out"], "{c}");
    }
    for c in f["isRef"].as_array().unwrap() {
        let s = c["in"].as_str().unwrap();
        assert_eq!((rows::is_ref(s), rows::ref_text(s)), (c["out"].as_bool().unwrap(), c["text"].as_str().unwrap().to_string()), "{c}");
    }
    for c in f["unitText"].as_array().unwrap() {
        assert_eq!(rows::unit_text(c["in"].as_str().unwrap()), c["out"].as_str().unwrap(), "{c}");
    }
}

#[test]
fn dates_and_sizes_as_the_web() {
    let f = fixtures();
    for c in f["parseYmd"].as_array().unwrap() {
        let got = date::parse_ymd(c["in"].as_str().unwrap(), 2026).map(|d| json!({ "y": d.y, "m": d.m, "d": d.d }));
        assert_eq!(got.unwrap_or(J::Null), c["out"], "{c}");
    }
    for c in f["detSize"].as_array().unwrap() {
        let (w, h) = det::det_size(c["in"][0].as_u64().unwrap() as usize, c["in"][1].as_u64().unwrap() as usize, 1920.0);
        assert_eq!(json!({ "w": w, "h": h }), c["out"], "{c}");
    }
}

#[test]
fn the_lexicon_translates_as_the_web() {
    for c in fixtures()["englishFor"].as_array().unwrap() {
        let name = c["in"].as_str().unwrap();
        assert_eq!(health_ocr::lexicon::english_for(name), c["out"].as_str().unwrap(), "{c}");
        let q: Vec<String> = serde_json::from_value(c["queries"].clone()).unwrap();
        assert_eq!(health_ocr::lexicon::queries_of(name), q, "{c}");
    }
}
