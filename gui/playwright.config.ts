import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: "tests",
  timeout: 30_000,
  use: { baseURL: "http://localhost:5199" },
  webServer: { command: "node tests/launch.mjs", url: "http://localhost:5199", reuseExistingServer: false, timeout: 60_000 },
});
