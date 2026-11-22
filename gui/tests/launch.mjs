// Playwright webServer: a throwaway profile with synthetic readings, a real `health serve`, and Vite in front of it.
import { execFileSync, spawn } from "node:child_process";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const bin = join(import.meta.dirname, "../../target/debug/health");
const dir = mkdtempSync(join(tmpdir(), "health-e2e-"));
const env = { ...process.env, HEALTH_DATA_DIR: join(dir, "data"), HEALTH_ROOT_KEY_FILE: join(dir, "root.key") };
execFileSync(bin, ["init"], { env });
const DAY = 86_400_000;
const now = Date.now();
const rows = [];
for (let d = 0; d < 40; d++) rows.push({ metric: "loinc:40443-4", start_ms: now - d * DAY - 3_600_000, tz_min: 0, value: 50 + (d % 7) });
writeFileSync(join(dir, "rows.jsonl"), rows.map((r) => JSON.stringify(r)).join("\n"));
execFileSync(bin, ["import", "rows", join(dir, "rows.jsonl"), "--source", "Test watch"], { env });
const serve = spawn(bin, ["serve"], { env, stdio: ["ignore", "pipe", "inherit"] });
const line = await new Promise((ok) => serve.stdout.once("data", (b) => ok(b.toString())));
writeFileSync(join(dir, "serve.json"), line);
process.env.E2E_DIR = dir;
const vite = spawn("npx", ["vite", "--port", "5199", "--strictPort"], { env: { ...process.env, HEALTH_SERVE: join(dir, "serve.json") }, stdio: "inherit" });
const stop = () => {
  serve.kill();
  vite.kill();
  process.exit(0);
};
process.on("SIGTERM", stop);
process.on("SIGINT", stop);
