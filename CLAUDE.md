# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

```sh
cargo build                                   # workspace (SQLCipher + vendored OpenSSL build on first run: slow)
cargo test --all                              # everything
cargo test -p health-core --test spec c13     # one spec check (tests are named c01..c24)
cargo test -p health-core --test parity       # web-backup parity
cargo test --release -p health-core --test scale -- --ignored --nocapture   # 1M-row perf; record results in docs/perf.md
cargo fmt --all && cargo clippy --all-targets -- -D warnings
scripts/check-no-net.sh                       # no networking crates outside health-connect
(cd gui && npm ci && npx playwright test)     # GUI end to end against a real `health serve` (builds nothing: run `cargo build -p health` first)
scripts/stage-core.sh release && (cd gui && npx tauri build)   # desktop bundle with the core inside
bun scripts/gen-catalog.ts                    # regenerate data/metrics.tsv from the web catalog
bun scripts/web-fixture.ts                    # regenerate tests/fixtures from the web code (needs the W repo)
bun scripts/web-import-check.ts <file> <pw>   # reverse parity: the web code restores a Rust-made backup (see the script header)
bun scripts/gen-ocr-data.ts                   # OCR tables and fixtures from the web code (reads tests/fixtures/<page>.web.json)
bun scripts/gen-zh-hant.ts                    # gui/src/locales/zh-Hant.json from zh-Hans.json (after editing zh-Hans)
bun scripts/gen-names-i18n.ts                 # Chinese measure names from the web code
(cd gui && node scripts/i18n.mjs)             # every UI text translated (also in CI)
HEALTH_OCR_MODELS=<dir> cargo test --all      # also runs the OCR parity/pipeline tests (pinned model files in <dir>)
scripts/demo-profile.sh /tmp/demo [<models>]  # a profile with synthetic data (screenshots, trying things)
```

Serve and MCP tests spawn processes. To find a hanging test, run the test binary under a process-group timeout, one test at a time (`target/debug/deps/serve-*  <name>`).

Run the CLI against a throwaway profile (no keychain prompts):

```sh
export HEALTH_DATA_DIR=/tmp/h HEALTH_ROOT_KEY_FILE=/tmp/h.key HEALTH_NOW=1795176000000 HEALTH_TZ_MIN=480
cargo run -q -- init && cargo run -q -- add loinc:2345-7 5.5 --unit mmol/L --at 2026-11-11T07:00 && cargo run -q -- --json obs latest
```

## Docs (read before changing behaviour)

- `docs/架構與實施計畫.md`: the plan (Traditional Chinese). §0.1 maps the reference files. Milestones are in §11.
- `docs/健康數據庫設計.md` + `docs/health-schema.sql`: the DB spec. Don't change the table structure. The schema is compiled in and must match the web version's `schema.sql` byte for byte.
- `docs/architecture.md`: how the code is laid out, the op table, the transaction rules and the formats.
- `docs/decisions.md`: judgment calls and **open questions for the owner**. Add a row whenever you decide something the specs don't cover.
- `docs/perf.md`: measured performance.

## Architecture in one screen

- `health-core` is the data layer. It never reads the clock or timezone (`now_ms`/`tz_min` are arguments), never touches the network, and calls no OS services (the key store is a trait). It is a rule-for-rule port of the web version's `src/client/lib/health/db/*`. If behaviour differs from the web, treat it as a bug unless it's logged in decisions.md.
- `health-api` is the operation table (`ops.rs`). CLI, HTTP, MCP and FFI all call `health_api::call()`, which runs validate → authorize → execute → audit. Add a feature as an op there, never as logic in the bin.
- Access (M2): `Caller::Owner | Caller::Delegate(grant)`. `OpDef.delegate` is the only list of what agents may call (deny by default), and the MCP tools are generated from it. Delegate reads go through `Scope` in SQL. Every delegate call is audited in `access.db`, and fails if the audit write fails. `health serve` (tiny_http, loopback) is the single writer; the CLI forwards to it when it is running.
- GUI (M4, `gui/`): the React UI calls `op()` in `gui/src/api.ts` only. Every text a person reads goes through `t()` (i18next, `gui/src/i18n.ts`; the English text is the key, `tk()` marks label tables translated where shown), with its Simplified translation in `gui/src/locales/zh-Hans.json`; regenerate zh-Hant from it. In Tauri that goes to the shell (`gui/src-tauri`), which runs `health serve --exit-with-parent`, holds the owner token and restarts the core. In the browser it goes through the Vite proxy. The UI never parses data rules. The design tokens are in `docs/gui.md`.
- OCR (M5): `health-ocr` ports the web `ocr/` (tract for the models, hayro for PDFs; same output as the web engine, tested on `crates/health-ocr/tests/fixtures`). It runs only in the `health ocr-worker` process, outside serve's profile lock; `health-core::ocr` records a review batch (invisible until `review.confirm_ocr`). Model files are a component under `<data dir>/components/ocr`, pinned by SHA-256.
- Account (M6): `health-proto` is the pure protocol (`B/docs/協議.md`): account keys from the AMK, JWS, HPKE, SBH1/SBHL1 codes, result checks. It is byte-identical to the web, proven on `tests/fixtures/web-account.json` (`bun scripts/gen-account-fixture.ts`) and the B vectors. The AMK is kept in the profile's `vault` (never in health.db, never in backups). Network ops (`health_api::account`, `call_net`) aren't in the op table: owner only, locking the profile only between requests. The link session lives in memory. Only the dev backend is built in. Debug builds take `HEALTH_BACKEND`/`HEALTH_LAB_KEYS`/`HEALTH_ATT_KEY` for test relays.
- `health-connect` is the only crate that opens network connections (component downloads, the health backend). `--offline` refuses both.
- `health` (bin) only parses arguments and owns the OS edges: the clock (`HEALTH_NOW`), the timezone (`HEALTH_TZ_MIN`, otherwise DST-aware per date), and key stores (the OS keychain, or `HEALTH_ROOT_KEY_FILE` at mode 0600).
- Key invariants that are easy to break:
  - Numbers enter storage only via `Value::number` (integral → INTEGER, as in JS).
  - `import.uid` decides row ownership ("larger uid wins"), so new uids must stay monotonic (`write::new_uid`).
  - Corrections are decided only by `CORRECTION_WINS` in SQL.
  - Review imports hold no rows, which keeps them invisible to queries.
  - Readers go through `v_observation` / `v_daily` only.
- Transactions: core functions nest with SAVEPOINTs. `call()` wraps write ops in `BEGIN IMMEDIATE` and deletes orphaned blob files only after COMMIT. ATTACH can't run inside a transaction, so `backup.*` ops are `manual_tx`.
- Profile directory: `id`, `health.db` (SQLCipher, raw key = HKDF(root)), `blobs/<sha256>` (chunked AES-GCM), `tmp/` (swept when a writer opens the profile), `lock` (writer only; readers don't lock).

## Reference repos (read-only, never modify)

- `W` = `~/Documents/GitHub/pre-pol/market/health`: the web version (TypeScript), which defines how things should behave.
- `B` = `~/Documents/GitHub/zuchiangmaiApp`: the protocol docs (`docs/協議.md`), test vectors and the OCR model list. Needed from M2/M5/M6 on.

CI never reads them. Commit whatever is derived from them: `data/metrics.tsv`, `tests/fixtures/*`, with the W commit recorded there.

## Hard rules

- Synthetic test data only. Never read personal exports or lab reports from `~/Downloads/`.
- Never send requests to the production backend. Use only the dev backend `https://api-dev-health.4seas.xyz`.
- Only `health-connect` may open network connections, which `scripts/check-no-net.sh` enforces. Logs never contain values, report text or passwords. Passwords never go in argv.
- License is GPL-3.0. New dependencies must pass `cargo deny check licenses` (`deny.toml`).
- Comments and docs are in English and concise. Don't over-engineer: create crates only when their milestone starts.
