// Playwright webServer: a throwaway profile with synthetic readings, a real `health serve`, and Vite in front of it.
import { execFileSync, spawn } from "node:child_process";
import { createServer } from "node:http";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const bin = join(import.meta.dirname, "../../target/debug/health");
const dir = mkdtempSync(join(tmpdir(), "health-e2e-"));

// A stand-in for the health backend (debug builds take HEALTH_BACKEND): inbox keys, the link relay, a request log.
const relay = { log: [], keys: new Set(), links: new Map() };
const backend = createServer((rq, rs) => {
  let body = "";
  rq.on("data", (c) => (body += c));
  rq.on("end", () => {
    const send = (status, v) => rs.writeHead(status, { "content-type": "application/json" }).end(JSON.stringify(v));
    const path = rq.url ?? "";
    if (path === "/_log") return send(200, { log: relay.log });
    relay.log.push(`${rq.method} ${path}`);
    if (rq.method === "POST" && path === "/h/1/key") {
      const payload = JSON.parse(Buffer.from(JSON.parse(body).jws.split(".")[1], "base64url").toString());
      relay.keys.add(payload.pk);
      return send(200, {});
    }
    const link = /^\/h\/1\/link\/([A-Z2-7]+)$/.exec(path)?.[1];
    if (link && rq.method === "POST") {
      if (relay.links.has(link)) return send(409, { err: "busy" });
      relay.links.set(link, JSON.parse(body).ct);
      return send(200, {});
    }
    if (link) {
      const ct = relay.links.get(link);
      relay.links.delete(link);
      return ct ? send(200, { ct }) : send(404, { err: "unknown" });
    }
    const inbox = /^\/h\/1\/inbox\/([^?]+)/.exec(path)?.[1];
    if (inbox) return relay.keys.has(inbox) ? send(200, { items: [], more: false }) : send(404, { err: "unknown" });
    send(404, { err: "unknown" });
  });
});
await new Promise((ok) => backend.listen(0, "127.0.0.1", ok));
const base = `http://127.0.0.1:${backend.address().port}`;
writeFileSync(join(tmpdir(), "health-e2e-relay.json"), JSON.stringify({ base, bin }));
const env = {
  ...process.env,
  HEALTH_DATA_DIR: join(dir, "data"),
  HEALTH_ROOT_KEY_FILE: join(dir, "root.key"),
  HEALTH_BACKEND: base,
  HEALTH_NO_BROWSER: "1",
  HEALTH_LAB_KEYS: "did:key:z6MkmpEvQWbkRbSWwtJyRbKK1qH4jCneVsuFt9knU8ZCdDsq:lab",
};
execFileSync(bin, ["init"], { env });
const DAY = 86_400_000;
const now = Date.now();
const rows = [];
for (let d = 0; d < 40; d++) rows.push({ metric: "loinc:40443-4", start_ms: now - d * DAY - 3_600_000, tz_min: 0, value: 50 + (d % 7) });
writeFileSync(join(dir, "rows.jsonl"), rows.map((r) => JSON.stringify(r)).join("\n"));
execFileSync(bin, ["import", "rows", join(dir, "rows.jsonl"), "--source", "Test watch"], { env });
// lab reports: the OCR models, when the test run has them (HEALTH_OCR_MODELS=<dir>)
if (process.env.HEALTH_OCR_MODELS) execFileSync(bin, ["component", "install", "ocr", "--from", process.env.HEALTH_OCR_MODELS], { env });
const serve = spawn(bin, ["serve"], { env, stdio: ["ignore", "pipe", "inherit"] });
const line = await new Promise((ok) => serve.stdout.once("data", (b) => ok(b.toString())));
writeFileSync(join(dir, "serve.json"), line);
process.env.E2E_DIR = dir;
const vite = spawn("npx", ["vite", "--port", "5199", "--strictPort"], { env: { ...process.env, HEALTH_SERVE: join(dir, "serve.json") }, stdio: "inherit" });
const stop = () => {
  serve.kill();
  vite.kill();
  backend.close();
  process.exit(0);
};
process.on("SIGTERM", stop);
process.on("SIGINT", stop);
