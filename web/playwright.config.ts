import { defineConfig, devices } from "@playwright/test";

/**
 * Playwright config for the web a11y/e2e suite.
 *
 * Phase 2 ships the wiring + a sign-in-page a11y baseline. Authenticated
 * walk-throughs (library → series → issue → reader → search) require a
 * seeded fixture DB and live in a follow-up; tracked under cross-cutting
 * tech debt in docs/dev/phase-status.md.
 */
export default defineConfig({
  testDir: "./tests/e2e",
  timeout: 30_000,
  retries: process.env.CI ? 2 : 0,
  reporter: process.env.CI ? "github" : "list",
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
      name: "chromium",
      use: { ...devices["Desktop Chrome"] },
    },
  ],
});
