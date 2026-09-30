/**
 * Shared axe-core assertion for the Playwright suite (§16.7, WP-4.8).
 *
 * One tag set for every surface so "passes axe" means the same thing on
 * the sign-in page and in the reader: WCAG 2.0/2.1 A + AA and the 2.2 AA
 * additions. Violations are reduced to `{id, impact, help, targets}` before
 * the assertion so a failure diff names the rule and the offending
 * selectors instead of dumping axe's full result objects.
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
): Promise<void> {
  const results = await new AxeBuilder({ page }).withTags(WCAG_TAGS).analyze();
  const violations = results.violations.map((v) => ({
    id: v.id,
    impact: v.impact,
    help: v.help,
    targets: v.nodes.map((n) => n.target.join(" ")),
  }));
  expect(violations, `axe violations: ${label}`).toEqual([]);
}
