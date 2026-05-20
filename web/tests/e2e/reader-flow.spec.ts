/**
 * Reader-flow E2E (deferred — see body for the scaffolding gap).
 *
 * The 1.0 readiness audit called for: register → land on Home → navigate
 * to first series → first issue → reader opens → arrow-key page advance
 * → progress writeback verified via the API.
 *
 * Implementing this faithfully needs a fixture harness the project
 * doesn't have yet:
 *
 *   1. A live Postgres + Redis (testcontainers or pre-started services).
 *   2. The Rust public-origin binary running at :8080 so `apiFetch` and
 *      `/issues/.../pages/...` actually answer. The current Playwright
 *      config points at `:3000` (Next.js directly) — fine for static-
 *      HTML degradation tests, useless for anything that touches the
 *      API.
 *   3. A seeded library so `register → first series` has somewhere to
 *      go. The scanner pipeline needs at least one CBZ archive on a
 *      mounted path plus a completed scan_run.
 *   4. Auth bootstrap: register the first user (auto-admin), grant
 *      library access, place a kebab-readable issue in their reach.
 *
 * Estimated effort: 4-6h once we decide on the fixture approach. The
 * cleanest option is probably a Rust `--seed-fixture <bundle.tar.gz>`
 * boot flag that hydrates a one-shot demo library on startup; the
 * Playwright runner then sets PLAYWRIGHT_BASE_URL to the Rust origin
 * and runs against that.
 *
 * Until then this spec stays `test.skip`-ed so `pnpm test:e2e` exits
 * clean. Removing the skip without the scaffolding listed above will
 * hang waiting for a non-existent series link.
 */
import { test, expect } from "@playwright/test";

test.describe.skip("Reader flow — deferred (needs fixture harness)", () => {
  test("register → first issue → page-turn → progress writeback", async ({
    page,
  }) => {
    // Placeholder steps documenting the intended walkthrough so the
    // contributor who lands the harness knows what to wire.
    const unique = `e2e-${Date.now()}@example.test`;
    await page.goto("/en/sign-in");
    // Click "Register" if the sign-in page has a CTA, or navigate
    // directly to /register.
    await page.goto("/en/register");
    await page.getByLabel(/email/i).fill(unique);
    await page.getByLabel(/password/i).fill("correctly-horse-battery");
    await page
      .getByRole("button", { name: /Create account|Register/i })
      .click();

    // First-user-admin bootstrap lands on Home. Pick the first series
    // card; the seed fixture guarantees at least one is visible.
    await page.waitForURL(/\/en\/?$/);
    const firstSeries = page.locator('[data-testid="series-card"]').first();
    await firstSeries.click();

    // Series detail → first issue.
    const firstIssue = page.locator('[data-testid="issue-card"]').first();
    await firstIssue.click();

    // Reader opens — chrome is off-by-default (UX batch),
    // so the page tap is what reveals chrome. We instead just press
    // ArrowRight to advance a page.
    await page.waitForURL(/\/read\//);
    await page.keyboard.press("ArrowRight");

    // Read back per-user progress and assert page > 0. Avoid coupling
    // to a specific page number — the assertion is "advance worked",
    // not "advanced N times".
    const issueId = page.url().match(/\/read\/[^/]+\/([^/?#]+)/)?.[1];
    expect(issueId).toBeTruthy();
    const progress = await page.request.get(
      `/api/me/issues/${issueId}/progress`,
    );
    expect(progress.ok()).toBeTruthy();
    const body = (await progress.json()) as { page: number };
    expect(body.page).toBeGreaterThan(0);
  });
});
