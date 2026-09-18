import { defineConfig, devices } from "@playwright/test";

/**
 * Playwright config for the web e2e suite.
 *
 * CI runs it in the `docker-smoke` job against the booted production images
 * through the Rust origin (PLAYWRIGHT_BASE_URL=http://localhost:8080); the
 * reader-flow spec needs that origin and skips elsewhere. Locally:
 * `just docker-build && just docker-e2e`. Without PLAYWRIGHT_BASE_URL the
 * config starts `next start` itself for the static specs.
 *
 * One retry in CI (not two): the suite gates dependency auto-merge, and a
 * retry can launder an intermittent regression into a pass — but browser-
 * level flakes are real and the whole run takes seconds.
 */
export default defineConfig({
  testDir: "./tests/e2e",
  timeout: 30_000,
  retries: process.env.CI ? 1 : 0,
  reporter: [
    [process.env.CI ? "github" : "list"],
    ["json", { outputFile: "test-results/results.json" }],
  ],
  use: {
    baseURL: process.env.PLAYWRIGHT_BASE_URL ?? "http://127.0.0.1:3000",
    trace: "retain-on-failure",
  },
  webServer: process.env.PLAYWRIGHT_BASE_URL
    ? undefined
    : {
        command: "pnpm run start",
        url: "http://127.0.0.1:3000",
        timeout: 60_000,
        reuseExistingServer: !process.env.CI,
      },
  projects: [
    {
      name: "mobile-chromium",
      testMatch: /pwa\.spec\.ts/,
      use: { ...devices["Pixel 7"] },
    },
    {
      name: "chromium",
      use: { ...devices["Desktop Chrome"] },
    },
  ],
});
