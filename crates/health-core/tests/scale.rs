//! Scale check (plan M1 acceptance, decision 13). Run: cargo test --release -p health-core --test scale -- --ignored --nocapture
//! 1M heart-rate rows written through the normal pipeline, encrypted vs plain on the same machine.

use health_core::Value;
use health_core::db::{self, Db};
use health_core::write::{Begin, ObsRow, SourceSpec, begin, finish, write};
use std::time::Instant;

const N: i64 = 1_000_000;
const STEP: i64 = 30_000; // one point every 30 s: 30 days = 86,400 points
const T0: i64 = 1_700_000_000_000;

fn fill(c: &Db) -> f64 {
    let t = Instant::now();
    let s = SourceSpec::new("healthkit", "watch");
    let (id, _) = begin(c, Begin { source: &s, parser: "bench", blob: None, uid: None, candidates: None }, T0).unwrap();
    for chunk in (0..N).collect::<Vec<_>>().chunks(100_000) {
        let rows: Vec<ObsRow> = chunk
            .iter()
            .map(|i| ObsRow {
                metric: "loinc:8867-4".into(),
                start_ms: T0 + i * STEP,
                end_ms: None,
                tz_min: 480,
                value: Value::Real(50.013 + (i % 997) as f64 * 0.137),
                unit: None,
                detail: None,
            })
            .collect();
        write(c, id, &rows).unwrap();
    }
    finish(c, id, T0).unwrap();
    t.elapsed().as_secs_f64()
}

fn query_30d(c: &Db) -> (usize, f64) {
    let mid = T0 + (N / 2) * STEP;
    let t = Instant::now();
    let mut st = c.prepare("SELECT start_ms, value FROM v_observation WHERE metric_id = (SELECT id FROM metric WHERE code = 'loinc:8867-4') AND start_ms >= ? AND start_ms < ?").unwrap();
    let n = st.query_map([mid, mid + 30 * 86_400_000], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, f64>(1)?))).unwrap().count();
    (n, t.elapsed().as_secs_f64() * 1000.0)
}

#[test]
#[ignore]
fn one_million_rows() {
    let dir = tempfile::tempdir().unwrap();
    let baseline_bytes = {
        let c = db::open_unencrypted(&dir.path().join("empty.db")).unwrap();
        c.execute_batch("VACUUM").unwrap();
        drop(c);
        std::fs::metadata(dir.path().join("empty.db")).unwrap().len()
    };
    for (name, enc) in [("plain", false), ("sqlcipher", true)] {
        let p = dir.path().join(format!("{name}.db"));
        let c = if enc { db::open(&p, &[9u8; 32]).unwrap() } else { db::open_unencrypted(&p).unwrap() };
        let write_s = fill(&c);
        c.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); VACUUM").unwrap();
        let bytes = std::fs::metadata(&p).unwrap().len() - baseline_bytes;
        let _ = query_30d(&c); // warm
        let (n, ms) = query_30d(&c);
        println!("{name:10} write {write_s:.1}s  {:.1} bytes/row  30-day query {n} rows {ms:.1} ms", bytes as f64 / N as f64);
        assert_eq!(n, 86_400);
        assert!(ms < 100.0, "30-day query must stay under 100 ms (plan §9)");
    }
}
