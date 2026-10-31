/**
 * Shared axe-core assertion for the Playwright suite (§16.7, WP-4.8).
 *
 * One tag set for every surface so "passes axe" means the same thing on
 * the sign-in page and in the reader: WCAG 2.0/2.1 A + AA and the 2.2 AA
 * additions. Violations are reduced to `{id, impact, help, targets}` before
 * the assertion so a failure diff names the rule and the offending
 * selectors instead of dumping axe's full result objects.
 *
 * `exclude` (CSS selectors) and `disableRules` (rule ids) are the
 * per-surface escape hatches for a violation that needs a design decision:
 * the caller names the finding next to the entry, every other element and
 * rule on the surface stays checked, and nothing changes elsewhere. Prefer
 * `exclude` — it leaves every rule running on the rest of the page.
 */
import { expect, type Page } from "@playwright/test";
import AxeBuilder from "@axe-core/playwright";

export const WCAG_TAGS = [
  "wcag2a",
  "wcag2aa",
  "wcag21a",
  "wcag21aa",
  "wcag22aa",
];

export async function expectNoAxeViolations(
  page: Page,
  label: string,
  options: { exclude?: string[]; disableRules?: string[] } = {},
): Promise<void> {
  // Let entry transitions (popover / dialog fade + zoom, tab slide) finish:
  // axe reads computed colours, and a half-faded element reports a
  // contrast that never exists at rest.
  await page.waitForFunction(() =>
    document
      .getAnimations()
      .every(
        (a) =>
          a.playState !== "running" ||
          a.effect?.getTiming().iterations === Infinity,
      ),
  );
  let builder = new AxeBuilder({ page }).withTags(WCAG_TAGS);
  for (const selector of options.exclude ?? []) {
    builder = builder.exclude(selector);
  }
  if (options.disableRules?.length) {
    builder = builder.disableRules(options.disableRules);
  }
  const results = await builder.analyze();
  const violations = results.violations.map((v) => ({
    id: v.id,
    impact: v.impact,
    help: v.help,
    targets: v.nodes.map((n) => n.target.join(" ")),
  }));
  expect(violations, `axe violations: ${label}`).toEqual([]);
}
