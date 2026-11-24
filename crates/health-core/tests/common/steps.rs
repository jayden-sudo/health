//! Random changes and table comparison shared by the sync tests (health-core's subset sync and health-api's
//! sessions, which include this file by path).
#![allow(dead_code)]

use health_core::correction::{self, Key};
use health_core::db::Db;
use health_core::manual::{self, Entry, When};
use health_core::profile::Profile;
use health_core::review::{self, Alias};
use health_core::write::{Begin, ObsRow, SourceSpec, begin, finish, write};
use health_core::{Value, erase, query};

pub const T0: i64 = 1_795_000_000_000;
pub const GLU: &str = "loinc:2345-7";
pub const RHR: &str = "loinc:40443-4";
pub const WEIGHT: &str = "loinc:29463-7";

/// xorshift64*: deterministic per seed, no dependency.
pub struct Rng(pub u64);
impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    pub fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    pub fn pick<'a, T>(&mut self, v: &'a [T]) -> Option<&'a T> {
        (!v.is_empty()).then(|| &v[self.below(v.len() as u64) as usize])
    }
}

pub fn value(r: &mut Rng) -> Value {
    match r.below(3) {
        0 => Value::Int(50 + r.below(10) as i64),
        1 => Value::Real(50.0 + r.below(10) as f64 + 0.5),
        _ => Value::Int(50),
    }
}

pub fn ids(c: &Db, sql: &str) -> Vec<i64> {
    c.prepare(sql).unwrap().query_map([], |r| r.get(0)).unwrap().collect::<Result<_, _>>().unwrap()
}

/// One random change on device `d` at time `now`. Refusals (a file already here, nothing to edit) are fine.
pub fn step(p: &Profile, d: usize, r: &mut Rng, now: i64) {
    let c = &p.db;
    let slot = |r: &mut Rng| T0 - 86_400_000 + r.below(4) as i64 * 3_600_000;
    match r.below(10) {
        0 | 1 => {
            let src = if r.below(2) == 0 { SourceSpec::new("whoop", "u1") } else { SourceSpec::new("lab", "dev1") };
            let (id, _) = begin(c, Begin { source: &src, parser: "t", blob: None, uid: None, candidates: None }, now).unwrap();
            let rows: Vec<ObsRow> = (0..1 + r.below(3))
                .map(|_| ObsRow {
                    metric: if r.below(2) == 0 { GLU.into() } else { RHR.into() },
                    start_ms: slot(r),
                    end_ms: None,
                    tz_min: 480,
                    value: value(r),
                    unit: None,
                    detail: (r.below(3) == 0).then(|| serde_json::json!({ "by": format!("d{d}") })),
                })
                .collect();
            write(c, id, &rows).unwrap();
            finish(c, id, now).unwrap();
        }
        2 => {
            if let Some(id) = r.pick(&ids(
                c,
                "SELECT i.id FROM import i JOIN source s ON s.id = i.source_id WHERE i.status = 'done' AND s.connector != 'manual'",
            )) {
                erase::erase(c, *id, now).unwrap();
            }
        }
        3 => {
            let e = Entry {
                metric: WEIGHT.into(),
                value: value(r),
                unit: Some("kg".into()),
                at: When::At { ms: slot(r), end_ms: None, tz_min: 480 },
            };
            let _ = manual::enter(c, &[e], now);
        }
        4 => {
            let at = slot(r);
            if r.below(2) == 0 {
                let e = Entry {
                    metric: WEIGHT.into(),
                    value: value(r),
                    unit: Some("kg".into()),
                    at: When::At { ms: at, end_ms: None, tz_min: 480 },
                };
                let _ = manual::edit(c, WEIGHT, at, &e, now);
            } else {
                let _ = manual::remove(c, WEIGHT, at, now);
            }
        }
        5 => {
            let rows: Vec<(i64, i64, i64)> = c
                .prepare(
                    "SELECT metric_id, source_id, start_ms FROM observation o JOIN metric m ON m.id = o.metric_id WHERE m.code IN (?, ?)",
                )
                .unwrap()
                .query_map([GLU, RHR], |x| Ok((x.get(0)?, x.get(1)?, x.get(2)?)))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap();
            if let Some(&(metric_id, source_id, start_ms)) = r.pick(&rows) {
                let v = if r.below(4) == 0 { Value::Null } else { value(r) };
                let at = correction::next_at(c, now).unwrap();
                correction::put(c, Key { metric_id, source_id, start_ms }, at, &v, now).unwrap();
            }
        }
        6 => {
            if let Some(id) = r.pick(&ids(c, "SELECT id FROM source")) {
                query::source_update(c, *id, None, Some(1 + r.below(200) as i64), now, &format!("d{d}")).unwrap();
            }
        }
        7 => {
            // a lab report waiting for review, then (maybe on another device after sync) confirmed differently
            if r.below(2) == 0 {
                let src = SourceSpec::new("ocr", "photo");
                let sha = if r.below(2) == 0 { "b".repeat(64) } else { "c".repeat(64) };
                let _ = begin(
                    c,
                    Begin { source: &src, parser: "ocr", blob: Some(&sha), uid: None, candidates: Some(serde_json::json!({"items": []})) },
                    now,
                );
            } else if let Some(id) = r.pick(&ids(c, "SELECT id FROM import WHERE status = 'review'")) {
                let rows = vec![ObsRow {
                    metric: GLU.into(),
                    start_ms: slot(r),
                    end_ms: None,
                    tz_min: 480,
                    value: value(r),
                    unit: None,
                    detail: None,
                }];
                let text = format!("report {}", r.below(3));
                review::confirm(c, *id, &rows, Some(&text), &[], now).unwrap();
            }
        }
        8 => {
            let code = if r.below(2) == 0 { GLU } else { "loinc:2339-0" };
            review::add_alias(c, &Alias { name: "glu".into(), unit: "mg/dL".into(), panel: None, code: code.into() }).unwrap();
        }
        _ => {
            // the same photo imported on several devices
            let src = SourceSpec::new("ocr", "photo");
            let sha = "d".repeat(64);
            if let Ok((id, _)) = begin(c, Begin { source: &src, parser: "ocr", blob: Some(&sha), uid: None, candidates: None }, now) {
                let rows = vec![ObsRow {
                    metric: GLU.into(),
                    start_ms: slot(r),
                    end_ms: None,
                    tz_min: 480,
                    value: value(r),
                    unit: None,
                    detail: None,
                }];
                write(c, id, &rows).unwrap();
                finish(c, id, now).unwrap();
            }
        }
    }
}

/// The tables by identity (never local ids), as one comparable text.
pub fn dump(c: &Db) -> String {
    let q = |sql: &str| -> Vec<String> {
        let mut st = c.prepare(sql).unwrap();
        let n = st.column_count();
        st.query_map([], |r| Ok((0..n).map(|i| format!("{:?}", r.get_ref(i).unwrap())).collect::<Vec<_>>().join("|")))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    };
    [
        q("SELECT m.code, s.connector, s.external_id, o.start_ms, i.uid, o.end_ms, o.tz_min, typeof(o.value), o.value, o.detail
             FROM observation o JOIN metric m ON m.id = o.metric_id JOIN source s ON s.id = o.source_id JOIN import i ON i.id = o.import_id
            ORDER BY 1, 2, 3, 4"),
        q("SELECT i.uid, i.status, i.blob, json_extract(i.meta, '$.file'), s.connector, s.external_id FROM import i JOIN source s ON s.id = i.source_id
            WHERE i.status IN ('done', 'review', 'erased') ORDER BY 1"),
        q("SELECT m.code, s.connector, s.external_id, k.start_ms, k.at, typeof(k.value), k.value
             FROM correction k JOIN metric m ON m.id = k.metric_id JOIN source s ON s.id = k.source_id ORDER BY 1, 2, 3, 4"),
        q("SELECT connector, external_id, name, priority, json_extract(meta, '$.set_at'), json_extract(meta, '$.set_by') FROM source ORDER BY 1, 2"),
        q("SELECT i.uid, t.text FROM report_text t JOIN import i ON i.id = t.rowid ORDER BY 1"),
        q("SELECT a.alias, a.unit, m.code FROM metric_alias a JOIN metric m ON m.id = a.metric_id ORDER BY 1, 2"),
    ]
    .map(|v| v.join("\n"))
    .join("\n--\n")
}

pub fn daily_text(c: &Db) -> String {
    let mut st =
        c.prepare("SELECT metric_id, day, source_id, n, quote(sum), quote(min), quote(max), seconds FROM daily ORDER BY 1, 2, 3").unwrap();
    let rows: Vec<String> = st
        .query_map([], |r| {
            Ok(format!(
                "{:?}",
                (
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, String>(6)?
                )
            ))
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    rows.join("\n")
}
