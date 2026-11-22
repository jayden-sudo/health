# Performance

`cargo test --release -p health-core --test scale -- --ignored --nocapture`

1M heart-rate rows (one every 30 s, `tz_min` = 480, non-integral values) written through the normal pipeline, measured after VACUUM, with the empty-schema size subtracted. The query reads 30 days through `v_observation`.

| Date | Machine | Store | Write 1M | Bytes/row | 30-day query (86,400 rows) |
| --- | --- | --- | --- | --- | --- |
| 2026-11-22 | Apple M1 Max, macOS 26 | plain SQLite 3.54 | 4.0 s | 29.3 | 10.7 ms |
| 2026-11-22 | Apple M1 Max, macOS 26 | SQLCipher 4.20 | 4.0 s | 29.7 | 15.5 ms |

DB design §8 (unencrypted SQLite 3.53, unknown hardware): 28.1 bytes/row and 39 ms. The query is well within that. Bytes/row is about 6% above it, and SQLCipher adds only 0.4 B/row: the gap is already in plain SQLite (other value distribution, `tz_min` = 480). Accepted by the owner (decision 13). An earlier run with `tz_min` = 0 and 10% integer values measured 27.0 B/row. The test fails above 100 ms (plan §9).

## Apple Health import (M3)

Synthetic export of 3,000,000 `<Record>` lines: heart rate every 2 minutes for about 5.7 years, plus step counts from a watch and a phone. The zip is 28 MB. Release build, `/usr/bin/time -l`:

| Date | Machine | Rows written | Time | Peak memory |
| --- | --- | --- | --- | --- |
| 2026-11-22 | Apple M1 Max | 1,700,001 (1.5M heart rate + 15-min step sums from 2 sources) | 12.9 s | 105 MB RSS |

Memory comes from the zip stream, one 5,000-row batch per source, and the 15-minute sums map (one entry per slot per cumulative type and source). It grows with years of cumulative data, not with the file size.
