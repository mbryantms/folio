/**
 * axe-core a11y smoke (§16.7).
 *
 * The public sign-in page is checked here. The reader pass (WP-4.8) runs
 * inside `reader-flow.spec.ts`, because that spec owns the only seeded
 * library: it registers the first user (the admin), scans the fixture and
 * opens the reader, and a second spec registering in parallel would race
 * it for the admin role. Both use the same assertion (`support/axe.ts`,
 * WCAG 2.2 AA tags):
 *
 *   - reader with the chrome hidden (the default state),
 *   - reader with the chrome shown,
 *   - reader with the page-text panel open.
 */
import { test } from "@playwright/test";
import { expectNoAxeViolations } from "./support/axe";

test.describe("Accessibility", () => {
  test("sign-in page has no WCAG 2.2 AA violations", async ({ page }) => {
    await page.goto("/sign-in");
    await page.waitForLoadState("networkidle");
    await expectNoAxeViolations(page, "sign-in");
  });
});
