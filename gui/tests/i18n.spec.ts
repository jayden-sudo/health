// The interface in three languages against a real core: the system's language is followed (zh-CN Simplified,
// zh-TW Traditional), the picker switches at once, measure names are translated, and no English sentence is
// left on any section in Chinese.
import { expect, test } from "@playwright/test";

const SECTIONS: Record<string, string[]> = {
  "zh-Hans": ["读数", "添加数据", "来源", "备份", "健康账户", "Agent 访问"],
  "zh-Hant": ["讀數", "添加數據", "來源", "備份", "健康賬戶", "Agent 訪問"],
};

/** Runs of three or more English words: an untranslated sentence. Product and file names are allowed. */
const ALLOWED = /Apple Health|Client (ID|Secret)|Claude (Desktop|Code)|VS Code|GitHub Copilot|Gemini CLI|Windsurf \/ Devin|LM Studio|JSON lines|Another MCP app/g;
const english = (text: string) => (text.replace(ALLOWED, "").match(/\b[A-Za-z]{2,}(?: [A-Za-z]{2,}){2,}\b/g) ?? []);

for (const [lang, locale] of [
  ["zh-Hans", "zh-CN"],
  ["zh-Hant", "zh-TW"],
] as const) {
  test.describe(`${lang} from the system (${locale})`, () => {
    test.use({ locale });

    test("every section is in Chinese, with Chinese measure names", async ({ page }) => {
      await page.goto("/");
      const nav = page.getByRole("navigation");
      await expect(page.locator("html")).toHaveAttribute("lang", lang);
      // the synthetic readings are resting heart rate (loinc:40443-4): the web version's name for it
      await expect(page.locator(".tile", { hasText: lang === "zh-Hans" ? "静息心率" : "靜息心率" })).toBeVisible();
      for (const name of SECTIONS[lang]!) {
        await nav.getByRole("button", { name, exact: true }).click();
        await expect(page.getByRole("heading", { level: 2 }).first()).toBeVisible();
        const text = await page.locator("main").innerText();
        expect(english(text), `${name}: ${text.slice(0, 300)}`).toEqual([]);
      }
    });
  });
}

test.describe("the language picker", () => {
  test.use({ locale: "en-US" });

  test("switches at once and is remembered", async ({ page }) => {
    await page.goto("/");
    await expect(page.getByRole("heading", { name: "Readings" })).toBeVisible();
    await page.getByLabel("Language").selectOption("zh-Hant");
    await expect(page.getByRole("heading", { name: "讀數" })).toBeVisible();
    await expect(page.locator("html")).toHaveAttribute("lang", "zh-Hant");
    await page.reload();
    await expect(page.getByRole("heading", { name: "讀數" })).toBeVisible();
    await page.getByLabel("語言").selectOption("zh-Hans");
    await expect(page.getByRole("heading", { name: "读数" })).toBeVisible();
    // back to the system's language
    await page.getByLabel("语言").selectOption("system");
    await expect(page.getByRole("heading", { name: "Readings" })).toBeVisible();
  });

  test("form errors read in the chosen language", async ({ page }) => {
    await page.goto("/");
    await page.getByLabel("Language").selectOption("zh-Hans");
    await page.getByRole("navigation").getByRole("button", { name: "备份", exact: true }).click();
    await page.getByLabel("密码", { exact: true }).fill("short");
    await page.getByLabel("再次输入密码", { exact: true }).fill("short");
    await page.getByRole("button", { name: /保存备份/ }).click();
    await expect(page.getByRole("alert")).toHaveText("至少 8 个字符。");
    await page.getByLabel("语言").selectOption("system");
  });
});
