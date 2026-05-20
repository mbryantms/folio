/**
 * Forgot-password disabled-banner E2E (Phase C C5).
 *
 * Validates that when the server's `/auth/config` reports
 * `password_recovery_enabled: false` (or is unreachable from the page's
 * SSR fetch), the page renders the "Email recovery is disabled" card
 * instead of the reset-link form. Without this gate, the form would
 * submit to a backend that silently fails — a real prod incident
 * waiting to happen on first-boot deployments where the operator
 * hasn't filled in SMTP yet.
 *
 * The page is a server component; it falls back to a safer default
 * (`password_recovery_enabled: false`) on any /auth/config error. We
 * lean on that fallback so this spec works against a bare `pnpm start`
 * (no Rust origin needed).
 */
import { test, expect } from "@playwright/test";
import AxeBuilder from "@axe-core/playwright";

test.describe("Forgot-password page", () => {
  test("renders disabled banner when recovery is not configured", async ({
    page,
  }) => {
    await page.goto("/en/forgot-password");
    await page.waitForLoadState("networkidle");

    // The disabled-banner card identifies itself by its heading copy.
    await expect(
      page.getByRole("heading", { name: "Email recovery is disabled" }),
    ).toBeVisible();

    // Body copy must guide the user toward an admin rather than the
    // form. We don't pin the exact phrasing — only that "administrator"
    // appears, so future copy edits don't require a test update.
    await expect(page.getByText(/administrator/i)).toBeVisible();

    // The reset-link form MUST NOT be on the page. If someone changes
    // the gate to render the form anyway, this regression-guards it.
    await expect(page.getByLabel(/^Email$/)).toHaveCount(0);
    await expect(
      page.getByRole("button", { name: /Send reset link/i }),
    ).toHaveCount(0);

    // "Back to sign-in" link is the only navigation affordance on the
    // disabled state — proves the user can exit without submitting.
    await expect(
      page.getByRole("link", { name: /Back to sign-in/i }),
    ).toBeVisible();
  });

  test("disabled-state page has no WCAG 2.2 AA violations", async ({
    page,
  }) => {
    await page.goto("/en/forgot-password");
    await page.waitForLoadState("networkidle");
    const results = await new AxeBuilder({ page })
      .withTags(["wcag2a", "wcag2aa", "wcag21a", "wcag21aa", "wcag22aa"])
      .analyze();
    expect(results.violations).toEqual([]);
  });
});
