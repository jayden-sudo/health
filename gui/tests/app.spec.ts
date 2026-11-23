// The UI against a real `health serve` (plan §6: Playwright, no Tauri needed).
import { expect, test } from "@playwright/test";
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

test("readings show the latest value with its trend", async ({ page }) => {
  await page.goto("/");
  const hero = page.locator(".tile", { hasText: "Resting heart rate" });
  await expect(hero).toContainText("/min");
  // the trend reads the value as the tile shows it, unit included
  await expect(hero.getByRole("img")).toHaveAttribute("aria-label", /latest \d+ \/min$/);
});

test("a reading entered by hand shows up in its history", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("navigation", { name: "Sections" }).getByRole("button", { name: "Data sources" }).click();
  await page.getByRole("button", { name: /^Enter by hand/ }).click();
  await page.getByPlaceholder("What did you measure?").fill("weight");
  await page.getByRole("button", { name: /^Weight/ }).first().click();
  await page.getByLabel("Value").fill("71.4");
  await page.getByLabel("Unit").selectOption("kg");
  await page.getByRole("button", { name: "Save reading" }).click();
  await expect(page.getByRole("heading", { name: "Weight", exact: true })).toBeVisible();
  await page.getByRole("button", { name: "Show table" }).click();
  await expect(page.getByRole("table").first()).toContainText("71.4");
});

test("agent access: the token is shown once and the grant can be revoked", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("navigation", { name: "Sections" }).getByRole("button", { name: "Agent access" }).click();
  await page.getByRole("button", { name: "Connect an app…" }).click();
  await page.getByRole("group", { name: "Agent apps" }).getByRole("button", { name: "Another MCP app" }).click();
  await page.getByLabel("Name shown in the access log").fill("Test agent");
  await page.getByRole("button", { name: "Allow access" }).click();
  await expect(page.locator(".connect pre").nth(2)).toHaveText(/^hk_[0-9a-f]{64}$/);
  await page.locator(".connect").getByRole("button", { name: "Done" }).click();
  await expect(page.locator(".connect")).toHaveCount(0);
  const row = page.getByRole("listitem", { name: /Test agent/ });
  await row.getByRole("button", { name: "Revoke" }).click();
  await row.getByRole("button", { name: "Revoke Test agent" }).click();
  await expect(row).toContainText("revoked");
});

test("a backup saves, and restoring it through Import changes nothing", async ({ page }) => {
  const file = join(tmpdir(), `e2e-${Date.now()}.zuhealth`);
  await page.goto("/");
  await page.getByRole("navigation", { name: "Sections" }).getByRole("button", { name: "Backup" }).click();
  await page.getByRole("button", { name: /^Save a backup/ }).click();
  await page.getByLabel("Password", { exact: true }).fill("correct horse");
  await page.getByLabel("Repeat password", { exact: true }).fill("correct horse");
  await page.getByLabel("Save to").fill(file);
  await page.getByRole("button", { name: "Save backup" }).click();
  await expect(page.getByRole("status")).toContainText("Saved");
  await page.keyboard.press("Escape");
  await page.getByRole("button", { name: /^Restore a backup/ }).click();
  await page.getByLabel("File path").fill(file);
  await expect(page.getByText("Health backup", { exact: true })).toBeVisible();
  await page.getByLabel("Backup password", { exact: true }).fill("wrong password");
  await page.getByRole("button", { name: "Restore backup" }).click();
  await expect(page.getByRole("alert")).toContainText("Wrong password");
  await page.getByLabel("Backup password", { exact: true }).fill("correct horse");
  await page.getByRole("button", { name: "Restore backup" }).click();
  await expect(page.getByRole("status")).toContainText("0 imports added");
});

test("import explains a file it can't read", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("navigation", { name: "Sections" }).getByRole("button", { name: "Data sources" }).click();
  await page.getByRole("button", { name: /^Lab report/ }).click();
  await page.getByLabel("File path").fill("/tmp/notes.txt");
  await expect(page.getByRole("alert")).toContainText("can't read this kind of file");
});

test("search narrows the cards to matching measures", async ({ page }) => {
  await page.goto("/");
  await page.getByLabel("Find a measure").fill("resting");
  await expect(page.locator(".section")).toHaveCount(1);
  await expect(page.locator(".section")).toContainText("Resting heart rate");
  await page.getByLabel("Find a measure").fill("zzz");
  await expect(page.getByText("No measure matches")).toBeVisible();
});

test("connecting an agent shows that app's own setup with the token in it", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("navigation", { name: "Sections" }).getByRole("button", { name: "Agent access" }).click();
  await page.getByRole("button", { name: "Connect an app…" }).click();
  await page.getByRole("group", { name: "Agent apps" }).getByRole("button", { name: "Codex" }).click();
  await expect(page.getByLabel("Name shown in the access log")).toHaveValue("Codex");
  await page.getByRole("button", { name: "Allow access" }).click();
  const steps = page.locator(".connect");
  await expect(steps.getByRole("heading", { name: "Connect Codex" })).toBeVisible();
  const cmd = await steps.locator("pre").first().textContent();
  expect(cmd).toMatch(/^codex mcp add health --env HEALTH_TOKEN=hk_[0-9a-f]{64} -- health mcp$/);
  const toml = await steps.locator("pre").nth(1).textContent();
  expect(toml).toContain('[mcp_servers.health.env]');
  await steps.getByRole("button", { name: "Done" }).click();
  await expect(page.getByRole("listitem", { name: /Codex/ })).toContainText("not used yet");
});

test("keyboard shortcuts switch sections and find focuses the search", async ({ page }) => {
  await page.goto("/");
  await expect(page.locator(".tiles").first()).toBeVisible();
  const mod = "Control"; // the UI takes Ctrl or ⌘ (Chromium keeps ⌘-digits for itself)
  await page.keyboard.press(`${mod}+3`);
  await expect(page.getByRole("heading", { name: "Backup", level: 2 })).toBeVisible();
  await page.keyboard.press(`${mod}+4`);
  await expect(page.getByRole("heading", { name: "Profile", level: 2 })).toBeVisible();
  await page.keyboard.press(`${mod}+f`);
  await expect(page.getByLabel("Find a measure")).toBeFocused();
});

test("history opens as a sheet over the readings and Escape closes it", async ({ page }) => {
  await page.goto("/");
  await page.locator(".tile", { hasText: "Resting heart rate" }).click();
  const sheet = page.getByRole("dialog", { name: "History" });
  await expect(sheet.getByRole("heading", { name: "Resting heart rate" })).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(sheet).toHaveCount(0);
  await expect(page.getByRole("heading", { name: "Readings", level: 2 })).toBeVisible();
});

test("closing the History sheet returns focus to the tile that opened it", async ({ page }) => {
  await page.goto("/");
  const tile = page.locator(".tile", { hasText: "Resting heart rate" });
  await tile.focus();
  await page.keyboard.press("Enter");
  await expect(page.getByRole("dialog", { name: "History" })).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(tile).toBeFocused();
});

test("a sheet opens with focus on itself, not on its close button", async ({ page }) => {
  await page.goto("/");
  await page.locator(".tile", { hasText: "Resting heart rate" }).click();
  const sheet = page.getByRole("dialog", { name: "History" });
  await expect(sheet).toBeFocused();
  await expect(sheet.getByRole("button", { name: "Close" })).not.toBeFocused();
});

test("a lab report photo is read, checked and saved", async ({ page }) => {
  test.skip(!process.env.HEALTH_OCR_MODELS, "needs HEALTH_OCR_MODELS");
  test.setTimeout(120_000);
  const report = join(import.meta.dirname, "../../crates/health-ocr/tests/fixtures/cbc.png");
  await page.goto("/");
  await page.getByRole("navigation", { name: "Sections" }).getByRole("button", { name: "Data sources" }).click();
  await page.getByRole("button", { name: /^Lab report/ }).click();
  await page.getByLabel("File path").fill(report);
  await expect(page.getByText("Lab report (photo or PDF)")).toBeVisible();
  await page.getByRole("dialog").getByRole("button", { name: "Import", exact: true }).click();
  await page.getByRole("button", { name: "Check them now" }).click({ timeout: 90_000 });
  const review = page.locator(".ocr");
  await expect(review.locator("tbody tr")).toHaveCount(14);
  await expect(review.getByLabel("Test date")).toHaveValue("2026-09-27");
  await expect(review.getByLabel("Measure for WBC")).not.toHaveValue("");
  // nothing counts until saved
  await review.getByLabel("Measure for Hemoglobin").selectOption("skip");
  await review.getByRole("button", { name: "Save 13 results" }).click();
  await expect(page.getByText(/Saved 13 results/)).toBeVisible();
  await page.getByRole("navigation", { name: "Sections" }).getByRole("button", { name: "Readings" }).click();
  await page.getByLabel("Find a measure").fill("platelet");
  await expect(page.locator(".tile", { hasText: "Platelets" })).toContainText("325");
});

test("a lab report PDF is read into a batch waiting for review", async ({ page }) => {
  test.skip(!process.env.HEALTH_OCR_MODELS, "needs HEALTH_OCR_MODELS");
  test.setTimeout(120_000);
  const report = join(import.meta.dirname, "../../crates/health-ocr/tests/fixtures/cbc.pdf");
  await page.goto("/");
  await page.getByRole("navigation", { name: "Sections" }).getByRole("button", { name: "Data sources" }).click();
  await page.getByRole("button", { name: /^Lab report/ }).click();
  await page.getByLabel("File path").fill(report);
  await expect(page.getByText("Lab report (photo or PDF)")).toBeVisible();
  await page.getByRole("dialog").getByRole("button", { name: "Import", exact: true }).click();
  await page.getByRole("button", { name: "Check them now" }).click({ timeout: 90_000 });
  const review = page.locator(".ocr");
  await expect(review.locator("tbody tr")).toHaveCount(14);
  await expect(review.getByLabel("Test date")).toHaveValue("2026-09-27");
  // discarded: nothing from it is kept
  await review.getByRole("button", { name: "Discard" }).click();
  await expect(page.getByText("Nothing is waiting.")).toBeVisible();
});

// ---- Health account (against the stand-in backend that tests/launch.mjs starts) ----------------------------

const relayInfo = () => JSON.parse(readFileSync(join(tmpdir(), "health-e2e-relay.json"), "utf8")) as { base: string; bin: string };
const relayLog = async () => ((await (await fetch(`${relayInfo().base}/_log`)).json()) as { log: string[] }).log;

test("the health account: nothing is sent until asked, then linking by check code gives the same identity", async ({ page }) => {
  test.setTimeout(90_000);
  // another computer that already has the account
  const { base, bin } = relayInfo();
  const other = mkdtempSync(join(tmpdir(), "health-e2e-other-"));
  const env = { ...process.env, HEALTH_DATA_DIR: join(other, "data"), HEALTH_ROOT_KEY_FILE: join(other, "root.key"), HEALTH_BACKEND: base };
  execFileSync(bin, ["init"], { env });
  const theirs = JSON.parse(execFileSync(bin, ["--json", "account", "register"], { env }).toString()) as { identity: string };

  const before = (await relayLog()).length;
  await page.goto("/");
  await page.getByRole("navigation", { name: "Sections" }).getByRole("button", { name: "Health account" }).click();
  await expect(page.getByText("There is no health account on this computer yet.")).toBeVisible();
  // the app is open and polling the inbox, yet without an account nothing went to the service
  expect((await relayLog()).length).toBe(before);

  await page.getByRole("button", { name: "Link an existing device…" }).click();
  const code = (await page.locator(".link-code").textContent())?.trim() ?? "";
  expect(code).toMatch(/^SBHL1:/);
  // the other computer hands it over (and answers yes to rule 2's question)
  const given = execFileSync(bin, ["--json", "link", "give", code], { env, input: "y\n" }).toString();
  const check = (JSON.parse(given) as { check: string }).check;
  await expect(page.locator(".check-code")).toHaveText(check, { timeout: 15_000 });
  await page.getByRole("button", { name: "They match" }).click();
  await expect(page.getByRole("heading", { name: "Your identity code" })).toBeVisible();
  const mine = await page.request.post("/v1/op/account.show", { data: {} });
  expect(((await mine.json()) as { value: { account: { identity: string } } }).value.account.identity).toBe(theirs.identity);
  // the link is in this device's own link history, with the check code both showed
  const history = page.getByRole("list", { name: "Link history" });
  await expect(history).toContainText("Received from another device");
  await expect(history).toContainText(`Check code ${check}`);
});

test("the profile: sex and birth month kept here, height recorded as a reading", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("navigation", { name: "Sections" }).getByRole("button", { name: "Profile" }).click();
  await page.getByRole("button", { name: /^Sex/ }).click();
  await page.getByRole("dialog").getByRole("radio", { name: "Female" }).click();
  await expect(page.getByRole("button", { name: /^Sex/ })).toContainText("Female");
  await page.getByRole("button", { name: /^Sex/ }).click();
  await expect(page.getByRole("dialog").getByRole("radio", { name: "Female" })).toHaveAttribute("aria-checked", "true");
  await page.keyboard.press("Escape");
  await page.getByRole("button", { name: /^Birth month/ }).click();
  await page.getByRole("combobox", { name: "Year" }).selectOption("1990");
  await page.getByRole("combobox", { name: "Month" }).selectOption("1");
  await page.getByRole("dialog").getByRole("button", { name: "Save" }).click();
  await expect(page.getByRole("button", { name: /^Birth month/ })).toContainText("1990");
  await expect(page.getByRole("button", { name: /^Birth month/ })).toContainText("years old");
  await page.getByRole("button", { name: /^Height/ }).click();
  await page.getByLabel("Height (cm)").fill("172.5");
  await page.getByRole("dialog").getByRole("button", { name: "Save" }).click();
  await expect(page.getByRole("button", { name: /^Height/ })).toContainText("172.5 cm");
  // the height is a reading like any other
  await page.getByRole("navigation", { name: "Sections" }).getByRole("button", { name: "Readings" }).click();
  await expect(page.locator(".tile", { hasText: "Height" })).toContainText("1.725");
});

test("manage data: a source lists its imports; one opens with what it holds and is deleted after asking twice", async ({ page }) => {
  await page.goto("/");
  const sections = page.getByRole("navigation", { name: "Sections" });
  await sections.getByRole("button", { name: "Data sources" }).click();
  await page.getByRole("button", { name: /^Enter by hand/ }).click();
  await page.getByPlaceholder("What did you measure?").fill("weight");
  await page.getByRole("button", { name: /^Weight/ }).first().click();
  await page.getByLabel("Value").fill("88.8");
  await page.getByLabel("Unit").selectOption("kg");
  await page.getByRole("button", { name: "Save reading" }).click();
  await expect(page.getByRole("heading", { name: "Weight", exact: true })).toBeVisible();
  await page.keyboard.press("Escape");
  await sections.getByRole("button", { name: "Data sources" }).click();
  await page.getByRole("button", { name: /^Manage data/ }).click();
  const sheet = page.getByRole("dialog");
  await sheet.getByRole("list", { name: "Manage data" }).getByRole("button", { name: /^Entered by you/ }).click();
  // newest first: the reading just entered
  await sheet.getByRole("list", { name: "Entered by you" }).getByRole("button").first().click();
  await expect(sheet).toContainText("1 entry");
  await expect(sheet.locator(".batch-items")).toContainText("88.8 kg");
  await sheet.getByRole("button", { name: "Delete this import" }).click();
  await expect(sheet.getByRole("group", { name: "Delete this import's data?" })).toContainText("1 entry will be deleted.");
  await sheet.getByRole("button", { name: "Delete", exact: true }).click();
  await expect(sheet.getByRole("group", { name: "Delete for good?" })).toBeVisible();
  await sheet.getByRole("button", { name: "Delete for good" }).click();
  await expect(sheet.getByRole("status")).toHaveText("Deleted.");
  await expect(sheet.getByRole("heading", { name: "Entered by you" })).toBeVisible();
  // cancelling the second question leaves everything as it was
  await sheet.getByRole("button", { name: "‹ Manage data" }).click();
  await sheet.getByRole("list", { name: "Manage data" }).getByRole("button").first().click();
  await sheet.getByRole("button", { name: /^Delete all/ }).click();
  await sheet.getByRole("button", { name: "Delete", exact: true }).click();
  await sheet.getByRole("button", { name: "Cancel" }).click();
  await expect(sheet.getByRole("button", { name: /^Delete all/ })).toBeVisible();
});

test("data sources: one row per source with its status; a table exports as CSV", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("navigation", { name: "Sections" }).getByRole("button", { name: "Data sources" }).click();
  await expect(page.getByRole("button", { name: /^WHOOP/ })).toContainText("Not connected");
  await page.getByRole("button", { name: /^Manage data/ }).click();
  await expect(page.getByRole("dialog").getByRole("heading", { name: "Manage data" })).toBeVisible();
  await page.keyboard.press("Escape");
  await page.getByRole("button", { name: /^WHOOP/ }).click();
  await expect(page.getByRole("dialog")).toContainText("http://localhost:47630/whoop");
  await page.keyboard.press("Escape");
  const file = join(tmpdir(), `e2e-${Date.now()}.csv`);
  await page.getByRole("navigation", { name: "Sections" }).getByRole("button", { name: "Backup" }).click();
  await page.getByRole("button", { name: /^Export a table/ }).click();
  await page.getByLabel("Save to").fill(file);
  await page.getByRole("button", { name: "Save table" }).click();
  await expect(page.getByRole("status")).toContainText("rows");
  expect(readFileSync(file, "utf8").split("\n").length).toBeGreaterThan(10);
});
