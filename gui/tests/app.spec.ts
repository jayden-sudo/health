// The UI against a real `health serve` (plan §6: Playwright, no Tauri needed).
import { expect, test } from "@playwright/test";
import { tmpdir } from "node:os";
import { join } from "node:path";

test("readings show the latest value with its trend", async ({ page }) => {
  await page.goto("/");
  const hero = page.locator(".tile", { hasText: "Resting heart rate" });
  await expect(hero).toContainText("/min");
  await expect(hero.getByRole("img")).toHaveAttribute("aria-label", /latest/);
});

test("a reading entered by hand shows up in its history", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("navigation", { name: "Sections" }).getByRole("button", { name: "Add data" }).click();
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
  await page.getByRole("group", { name: "Agent apps" }).getByRole("button", { name: "Another MCP app" }).click();
  await page.getByLabel("Name shown in the access log").fill("Test agent");
  await page.getByRole("button", { name: "Create access" }).click();
  await expect(page.locator(".connect pre").nth(2)).toHaveText(/^hk_[0-9a-f]{64}$/);
  await page.locator(".connect").getByRole("button", { name: "Done" }).click();
  await expect(page.locator(".connect")).toHaveCount(0);
  const row = page.getByRole("row", { name: /Test agent/ });
  await row.getByRole("button", { name: "Revoke" }).click();
  await row.getByRole("button", { name: "Revoke Test agent" }).click();
  await expect(row).toContainText("revoked");
});

test("a backup saves, and restoring it through Import changes nothing", async ({ page }) => {
  const file = join(tmpdir(), `e2e-${Date.now()}.zuhealth`);
  await page.goto("/");
  await page.getByRole("navigation", { name: "Sections" }).getByRole("button", { name: "Backup" }).click();
  await page.getByLabel("Password", { exact: true }).fill("correct horse");
  await page.getByLabel("Repeat password", { exact: true }).fill("correct horse");
  await page.getByLabel("Save to").fill(file);
  await page.getByRole("button", { name: "Save backup" }).click();
  await expect(page.getByRole("status")).toContainText("Saved");
  await page.getByRole("navigation", { name: "Sections" }).getByRole("button", { name: "Add data" }).click();
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
  await page.getByRole("navigation", { name: "Sections" }).getByRole("button", { name: "Add data" }).click();
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
  await page.getByRole("group", { name: "Agent apps" }).getByRole("button", { name: "Codex" }).click();
  await expect(page.getByLabel("Name shown in the access log")).toHaveValue("Codex");
  await page.getByRole("button", { name: "Create access" }).click();
  const steps = page.locator(".connect");
  await expect(steps.getByRole("heading", { name: "Connect Codex" })).toBeVisible();
  const cmd = await steps.locator("pre").first().textContent();
  expect(cmd).toMatch(/^codex mcp add health --env HEALTH_TOKEN=hk_[0-9a-f]{64} -- health mcp$/);
  const toml = await steps.locator("pre").nth(1).textContent();
  expect(toml).toContain('[mcp_servers.health.env]');
  await steps.getByRole("button", { name: "Done" }).click();
  await expect(page.getByRole("row", { name: /Codex/ })).toContainText("not used yet");
});

test("keyboard shortcuts switch sections and find focuses the search", async ({ page }) => {
  await page.goto("/");
  await expect(page.locator(".tiles").first()).toBeVisible();
  const mod = "Control"; // the UI takes Ctrl or ⌘ (Chromium keeps ⌘-digits for itself)
  await page.keyboard.press(`${mod}+4`);
  await expect(page.getByRole("heading", { name: "Backup", level: 2 })).toBeVisible();
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

test("a lab report photo is read, checked and saved", async ({ page }) => {
  test.skip(!process.env.HEALTH_OCR_MODELS, "needs HEALTH_OCR_MODELS");
  test.setTimeout(120_000);
  const report = join(import.meta.dirname, "../../crates/health-ocr/tests/fixtures/cbc.png");
  await page.goto("/");
  await page.getByRole("navigation", { name: "Sections" }).getByRole("button", { name: "Add data" }).click();
  await page.getByLabel("File path").fill(report);
  await expect(page.getByText("Lab report (photo or PDF)")).toBeVisible();
  await page.getByRole("button", { name: "Import" }).click();
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
  await page.getByRole("navigation", { name: "Sections" }).getByRole("button", { name: "Add data" }).click();
  await page.getByLabel("File path").fill(report);
  await expect(page.getByText("Lab report (photo or PDF)")).toBeVisible();
  await page.getByRole("button", { name: "Import" }).click();
  await page.getByRole("button", { name: "Check them now" }).click({ timeout: 90_000 });
  const review = page.locator(".ocr");
  await expect(review.locator("tbody tr")).toHaveCount(14);
  await expect(review.getByLabel("Test date")).toHaveValue("2026-09-27");
  // discarded: nothing from it is kept
  await review.getByRole("button", { name: "Discard" }).click();
  await expect(page.getByText("Nothing is waiting.")).toBeVisible();
});
