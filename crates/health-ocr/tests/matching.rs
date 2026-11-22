//! Measure matching gives the web's candidates and preselection on every test page's items (catalog only).
//! Needs the embedding model: HEALTH_OCR_MODELS=<dir with e5_*>; skipped without.

use health_core::ocr::OcrItem;
use health_ocr::embed::Embedder;
use health_ocr::matcher::{match_items, preselected};
use serde_json::Value as J;

#[test]
fn same_candidates_as_the_web_on_every_page() {
    let Some(dir) = std::env::var_os("HEALTH_OCR_MODELS").map(std::path::PathBuf::from) else {
        eprintln!("skipped: set HEALTH_OCR_MODELS");
        return;
    };
    let e = Embedder::new(&std::fs::read(dir.join("e5_model_int8.onnx")).unwrap(), &std::fs::read(dir.join("e5_tokenizer.json")).unwrap())
        .unwrap();
    let probe = e.embed(&["Hemoglobin".into(), "白细胞计数".into()], "query").unwrap();
    let web: J = serde_json::from_str(include_str!("fixtures/web-match.json")).unwrap();
    for (v, w) in probe.iter().zip(web["probe"].as_array().unwrap()) {
        for (a, b) in v.iter().zip(w.as_array().unwrap()) {
            assert!((*a as f64 - b.as_f64().unwrap()).abs() < 2e-3, "embedding {a} vs {b}");
        }
    }
    let tables: J = serde_json::from_str(include_str!("fixtures/web-ocr.json")).unwrap();
    for (page, t) in tables["tables"].as_object().unwrap() {
        let items: Vec<OcrItem> = serde_json::from_value(t["items"].clone()).unwrap();
        let got = match_items(&e, &items).unwrap();
        for (k, (g, w)) in got.iter().zip(web["pages"][page].as_array().unwrap()).enumerate() {
            let want: Vec<(String, f64)> = w["suggest"]
                .as_array()
                .unwrap()
                .iter()
                .map(|s| (s["code"].as_str().unwrap().to_string(), s["score"].as_f64().unwrap()))
                .collect();
            // tract and onnxruntime quantise the int8 model slightly differently (scores within 0.003): the first
            // candidate must agree; others may differ only where they sit at the cut-off (decision 52)
            assert_eq!(g.first().map(|s| s.code.as_str()), want.first().map(|w| w.0.as_str()), "{page} item {k} {}", w["name"]);
            let near_cut = |score: f64| (score - health_ocr::matcher::SUGGEST_MIN).abs() <= 0.005;
            for s in g {
                match want.iter().find(|w| w.0 == s.code) {
                    Some((_, score)) => assert!((s.score - score).abs() <= 0.003, "{page} item {k}: {} {} vs {score}", s.code, s.score),
                    None => assert!(near_cut(s.score), "{page} item {k}: extra {} at {}", s.code, s.score),
                }
            }
            for (code, score) in &want {
                if !g.iter().any(|s| &s.code == code) {
                    assert!(near_cut(*score), "{page} item {k}: missing {code} at {score}");
                }
            }
            assert_eq!(preselected(g).map(|s| s.code.as_str()), w["pre"].as_str(), "{page} item {k} preselection");
        }
    }
}
