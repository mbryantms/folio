/**
 * axe-core a11y smoke (§16.7).
 *
 * The public sign-in page is checked here. The reader pass (WP-4.8) runs
 * inside `reader-flow.spec.ts`, because that spec owns the reader state
 * (it opens the fixture issue and turns pages). Both use the same
 * assertion (`support/axe.ts`, WCAG 2.2 AA tags):
 *
 *   - reader with the chrome hidden (the default state),
 *   - reader with the chrome shown,
 *   - reader with the page-text panel open.
 *
 * The M7b relationship surfaces (WP-8.5) are checked inside
 * `relationship-review.spec.ts`, which owns the fixture's relationship
 * suggestion: the series Related tab (pending chip; then the link, the
 * reading-order strip and the Similar rail), `/admin/relationships` (the
 * pending list and the "Edit kind" popover) and the "Add relationship"
 * dialog with the kind picker open. Both specs start from the shared admin
 * session (`admin.setup.ts`) instead of registering.
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
