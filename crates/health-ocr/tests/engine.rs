//! The engine gives the web engine's text, box for box, on every test page (same models, same pre- and
//! post-processing). Needs the models: HEALTH_OCR_MODELS=<dir with ppocrv5_*>; skipped without.

use health_ocr::engine::Engine;
use health_ocr::types::Img;
use serde_json::Value as J;

#[test]
fn same_lines_as_the_web_engine_on_every_page() {
    let Some(dir) = std::env::var_os("HEALTH_OCR_MODELS").map(std::path::PathBuf::from) else {
        eprintln!("skipped: set HEALTH_OCR_MODELS");
        return;
    };
    let read = |f: &str| std::fs::read(dir.join(f)).unwrap();
    let engine =
        Engine::new(&read("ppocrv5_det.onnx"), &read("ppocrv5_rec.onnx"), &String::from_utf8(read("ppocrv5_dict.txt")).unwrap()).unwrap();
    // the committed pages, or a larger set elsewhere (HEALTH_OCR_SAMPLES=<dir of *.png with *.web.json>)
    let fx = std::env::var("HEALTH_OCR_SAMPLES").unwrap_or_else(|_| format!("{}/tests/fixtures", env!("CARGO_MANIFEST_DIR")));
    let mut pages: Vec<String> =
        std::fs::read_dir(&fx).unwrap().filter_map(|e| e.ok()?.file_name().into_string().ok()).filter(|n| n.ends_with(".png")).collect();
    pages.sort();
    assert!(pages.len() >= 7);
    for page in pages {
        let png = image::open(format!("{fx}/{page}")).unwrap().to_rgba8();
        let img = Img { width: png.width() as usize, height: png.height() as usize, data: png.into_raw() };
        let t = std::time::Instant::now();
        let lines = engine.recognize(&img, &mut |_, _| {}).unwrap();
        eprintln!("{page}: {} lines in {:?}", lines.len(), t.elapsed());
        let web: Vec<J> =
            serde_json::from_str(&std::fs::read_to_string(format!("{fx}/{}", page.replace(".png", ".web.json"))).unwrap()).unwrap();
        let got: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
        let want: Vec<&str> = web.iter().map(|l| l["text"].as_str().unwrap()).collect();
        assert_eq!(got, want, "{page}");
        for (l, w) in lines.iter().zip(&web) {
            assert!((l.bx.x - w["box"]["x"].as_f64().unwrap()).abs() < 1e-6, "{page}: {} box", l.text);
            assert!((l.score - w["score"].as_f64().unwrap()).abs() < 2e-3, "{page}: {} score", l.text);
        }
    }
}
