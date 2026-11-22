// Reverse parity: the web code restores a backup made by the Rust CLI and must compute the same v_daily
// as tests/fixtures/web-expected.json. Local check only (needs the W repo).
// Usage: bun scripts/web-import-check.ts <backup.zuhealth> <password> [web repo path]
import { Database } from "bun:sqlite";
import { plugin } from "bun";
import { mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { dirname, join } from "node:path";

const [file, password] = process.argv.slice(2);
const web = process.argv[4] ?? join(homedir(), "Documents/GitHub/pre-pol/market/health");
plugin({
  name: "raw",
  setup(b) {
    b.onResolve({ filter: /\?raw$/ }, (a) => ({ path: join(dirname(a.importer), a.path.replace(/\?raw$/, "")), namespace: "raw" }));
    b.onLoad({ filter: /.*/, namespace: "raw" }, (a) => ({ contents: `export default ${JSON.stringify(readFileSync(a.path, "utf8"))}`, loader: "js" }));
  },
});
const lib = join(web, "src/client/lib/health");
const { txPair } = await import(join(lib, "db/engine.ts"));
const { initDb } = await import(join(lib, "db/open.ts"));
const { mergeFrom } = await import(join(lib, "db/merge.ts"));
const { unpackExport, collect } = await import(join(lib, "export/pack.ts"));

const tmp = join(tmpdir(), `health-revcheck-${process.pid}`);
mkdirSync(tmp, { recursive: true });
const files = await collect(unpackExport([new Uint8Array(readFileSync(file!))], password!));
writeFileSync(join(tmp, "plain.db"), files.get("db.sqlite")!);
const raw = new Database(join(tmp, "web.db"));
const bind = (p: readonly unknown[] = []) => p.map((v) => (typeof v === "number" && Number.isSafeInteger(v) ? BigInt(v) : v)) as never[];
const db = {
  exec: (sql: string, p?: readonly unknown[]) => (p?.length ? raw.query(sql).run(...bind(p)) : raw.exec(sql)),
  all: (sql: string, p?: readonly unknown[]) => raw.query(sql).all(...bind(p)),
  prepare: (sql: string) => {
    const st = raw.query(sql);
    return { run: (p?: unknown[]) => void st.run(...bind(p)), all: (p?: unknown[]) => st.all(...bind(p)), finalize() {} };
  },
  ...txPair((s: string) => raw.exec(s)),
};
initDb(db);
const expected = JSON.parse(readFileSync(new URL("../crates/health-core/tests/fixtures/web-expected.json", import.meta.url), "utf8"));
const rep = mergeFrom(db, join(tmp, "plain.db"), { now: expected.now });
const daily = db.all(`SELECT m.code, d.day, s.connector, s.external_id, d.n, d.sum, d.min, d.max, d.seconds
  FROM v_daily d JOIN metric m ON m.id = d.metric_id JOIN source s ON s.id = d.source_id ORDER BY m.code, d.day`);
const big = (_: string, v: unknown) => (typeof v === "bigint" ? Number(v) : v);
const same = JSON.stringify(daily, big) === JSON.stringify(expected.daily);
raw.close();
rmSync(tmp, { recursive: true, force: true });
console.log(`skipped metrics: ${rep.skippedMetrics.length}, daily rows: ${daily.length}, identical to web-expected: ${same}`);
process.exit(same && rep.skippedMetrics.length === 0 ? 0 : 1);
