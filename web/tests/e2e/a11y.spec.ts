/**
 * axe-core a11y smoke (§16.7).
 *
 * Phase 2 ships a baseline against the public sign-in page. The full
 * library → series → issue → reader walk lands once the e2e fixture
 * harness is in place (deferred — see phase-status.md).
 */
import { test, expect } from "@playwright/test";
import AxeBuilder from "@axe-core/playwright";

test.describe("Accessibility", () => {
  test("sign-in page has no WCAG 2.2 AA violations", async ({ page }) => {
    await page.goto("/en/sign-in");
    await page.waitForLoadState("networkidle");
    const results = await new AxeBuilder({ page })
      .withTags(["wcag2a", "wcag2aa", "wcag21a", "wcag21aa", "wcag22aa"])
      .analyze();
    expect(results.violations).toEqual([]);
  });
});
