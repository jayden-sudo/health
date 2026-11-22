//! M3 parity: the Rust Apple parser and WHOOP decoder give the same rows as the web code on the same input
//! (fixtures from scripts/web-fixture-m3.ts), plus the importers end to end.

use health_core::{apple, db, erase, import, query, scope::Scope, whoop};
use serde_json::{Value, json};
use std::io::Write;

const FIX: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

fn fixture(name: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(format!("{FIX}/{name}")).unwrap()).unwrap()
}

fn zip_of(dir: &std::path::Path, name: &str, xml: &str) -> std::path::PathBuf {
    let p = dir.join(name);
    let mut z = zip::ZipWriter::new(std::fs::File::create(&p).unwrap());
    let o = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    z.start_file("apple_health_export/export_cda.xml", o).unwrap();
    z.write_all(b"<ClinicalDocument/>").unwrap();
    z.start_file("apple_health_export/export.xml", o).unwrap();
    z.write_all(xml.as_bytes()).unwrap();
    z.finish().unwrap();
    p
}

#[test]
fn apple_rows_match_the_web_parser() {
    let xml = std::fs::read(format!("{FIX}/apple-export.xml")).unwrap();
    let mut rows = Vec::new();
    let stats = apple::parse(&xml[..], &mut |(source, row)| {
        let mut v = serde_json::to_value(&row).unwrap();
        v["source"] = json!(source);
        rows.push(v);
    })
    .unwrap();
    let want = fixture("apple-expected.json");
    assert_eq!(Value::Array(rows), want["rows"]);
    assert_eq!(stats.records, want["stats"]["records"].as_u64().unwrap());
    assert_eq!(stats.mapped, want["stats"]["mapped"].as_u64().unwrap());
    assert_eq!(stats.broken, want["stats"]["broken"].as_u64().unwrap());
    assert_eq!(serde_json::to_value(&stats.skipped_types).unwrap(), want["stats"]["skippedTypes"]);
}

#[test]
fn whoop_rows_match_the_web_decoder() {
    let want = fixture("whoop-expected.json");
    let bundle: whoop::WhoopData = serde_json::from_value(fixture("whoop-bundle.json")).unwrap();
    let got = serde_json::to_value(whoop::decode(&bundle, want["now"].as_i64())).unwrap();
    // `end_ms: null` (web) and an absent end_ms (Rust) both mean "no end"
    let mut rows = want["rows"].clone();
    for r in rows.as_array_mut().unwrap() {
        if r["end_ms"].is_null() {
            r.as_object_mut().unwrap().remove("end_ms");
        }
    }
    assert_eq!(got, rows);
}

#[test]
fn an_apple_zip_imports_once_per_device_and_reimports_without_duplicates() {
    let d = tempfile::tempdir().unwrap();
    let xml = std::fs::read_to_string(format!("{FIX}/apple-export.xml")).unwrap();
    let z = zip_of(d.path(), "export.zip", &xml);
    let c = db::open_memory().unwrap();
    let r = import::apple(&c, &z, 0).unwrap();
    assert_eq!((r.written, r.added, r.sources), (18, 18, 6), "{r:?}");
    let again = import::apple(&c, &z, 0).unwrap();
    assert_eq!((again.written, again.added), (18, 0));
    // steps: watch and phone both report the same slot; the daily summary elects one, never the sum
    let steps = query::daily(&c, &Scope::all(), "loinc:55423-8", None, None).unwrap();
    assert_eq!(steps.len(), 1);
    assert!(steps[0].sum == Some(500.0) || steps[0].sum == Some(500.0 + 0.0), "{:?}", steps[0]);
    assert!(import::apple(&c, &d.path().join("nope.zip"), 0).is_err());
    std::fs::write(d.path().join("bad.zip"), b"not a zip").unwrap();
    assert_eq!(import::apple(&c, &d.path().join("bad.zip"), 0).unwrap_err().code, "not_apple");
}

/// DB design §5.4 with real exports: rows belong to the newest export that delivered them.
#[test]
fn overlapping_apple_exports() {
    let d = tempfile::tempdir().unwrap();
    let hr = |n: usize| {
        let recs: String = (0..n)
            .map(|i| format!(" <Record type=\"HKQuantityTypeIdentifierHeartRate\" sourceName=\"W\" unit=\"count/min\" startDate=\"2026-11-18 07:0{i}:00 +0800\" endDate=\"2026-11-18 07:0{i}:00 +0800\" value=\"6{i}\"/>\n"))
            .collect();
        format!("<HealthData>\n{recs}</HealthData>\n")
    };
    let (old, new) = (zip_of(d.path(), "old.zip", &hr(3)), zip_of(d.path(), "new.zip", &hr(5)));
    let import_of = |c: &db::Db, uid_order: usize| -> i64 {
        c.query_row("SELECT id FROM import WHERE status = 'done' ORDER BY uid LIMIT 1 OFFSET ?", [uid_order as i64], |r| r.get(0)).unwrap()
    };
    let visible = |c: &db::Db| c.query_row("SELECT count(*) FROM v_observation", [], |r| r.get::<_, i64>(0)).unwrap();
    for (erase_newer, left) in [(false, 5), (true, 0)] {
        let c = db::open_memory().unwrap();
        import::apple(&c, &old, 1).unwrap();
        import::apple(&c, &new, 2).unwrap();
        let id = import_of(&c, erase_newer as usize);
        erase::erase(&c, id, 3).unwrap();
        assert_eq!(visible(&c), left, "erase newer = {erase_newer}");
    }
}

#[test]
fn whoop_bundle_and_row_files_import() {
    let d = tempfile::tempdir().unwrap();
    let c = db::open_memory().unwrap();
    let bundle: whoop::WhoopData = serde_json::from_value(fixture("whoop-bundle.json")).unwrap();
    let r = import::whoop(&c, &bundle, fixture("whoop-expected.json")["now"].as_i64().unwrap()).unwrap();
    assert_eq!((r.written, r.sources), (27, 1));
    let f = d.path().join("rows.jsonl");
    std::fs::write(&f, "{\"metric\":\"loinc:8867-4\",\"start_ms\":1,\"tz_min\":0,\"value\":60}\n\n{\"metric\":\"loinc:29463-7\",\"start_ms\":1,\"tz_min\":0,\"value\":150,\"unit\":\"[lb_av]\"}\n").unwrap();
    let r = import::rows(&c, &f, "Lab export", 2).unwrap();
    assert_eq!((r.written, r.sources), (2, 1));
    std::fs::write(&f, "{\"metric\":1}\n").unwrap();
    assert_eq!(import::rows(&c, &f, "x", 3).unwrap_err().code, "bad_row");
}

#[test]
fn the_csv_carries_values_but_never_identifiers_or_unnamed_sensitive_groups() {
    let c = db::open_memory().unwrap();
    let bundle: whoop::WhoopData = serde_json::from_value(fixture("whoop-bundle.json")).unwrap();
    import::whoop(&c, &bundle, fixture("whoop-expected.json")["now"].as_i64().unwrap()).unwrap();
    let e = |metric: &str, v: Value| health_core::manual::Entry {
        metric: metric.into(),
        value: serde_json::from_value(v).unwrap(),
        unit: None,
        at: health_core::manual::When::Date { date: "2026-11-18".into(), tz_min: 480 },
    };
    health_core::manual::enter(&c, &[e("loinc:22322-2", json!("=HYPERLINK(\"x\")"))], 1).unwrap(); // infectious (sensitive), formula-like
    let (csv, n) =
        health_core::csv::build(&c, i64::MIN, &["sleep".into(), "cardio".into(), "body".into(), "activity".into()], false).unwrap();
    assert!(csv.starts_with('\u{feff}') && csv.ends_with("\r\n"));
    assert_eq!(csv.lines().count(), n + 1);
    for secret in ["whoop-user-1", "\"whoop\"", "raw", "HYPERLINK"] {
        assert!(!csv.contains(secret), "{secret} leaked");
    }
    assert!(csv.contains("x:daily_deep_sleep") && csv.contains(",90,min,")); // 5,400,000 ms of deep sleep → 90 min
    let (with, _) = health_core::csv::build(&c, i64::MIN, &["infectious".into()], true).unwrap();
    assert!(with.contains("'=HYPERLINK") && !with.contains(":00,+08:00")); // defused; date only
    let g = health_core::csv::groups(&c, i64::MIN).unwrap();
    assert!(g.iter().any(|x| x.group == "infectious" && x.sensitive && x.rows == 1));
}
