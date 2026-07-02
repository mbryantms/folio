/**
 * CSP regression smoke (frontend-audit I1).
 *
 * The Rust origin (:8080) serves every page with a per-request
 * `script-src 'nonce-…' 'strict-dynamic'` policy and forwards that
 * header on the proxy hop so Next nonces its framework scripts and
 * the root layout nonces next-themes' inline bootstrap. A regression
 * anywhere in that chain (proxy header dropped, layout nonce
 * extraction broken, a new un-nonced inline script introduced)
 * surfaces as a console violation on every navigation — which is
 * exactly what this spec asserts against.
 *
 * Run against the Rust origin for the assertion to be meaningful:
 *   PLAYWRIGHT_BASE_URL=http://localhost:8080 pnpm exec playwright test csp
 * Pointed at raw Next (:3000, the harness default) there is no CSP
 * to enforce, so the spec skips rather than passing vacuously.
 */
import { test, expect } from "@playwright/test";

test.describe("Content Security Policy", () => {
  test("sign-in page loads with no CSP violations and fully nonced inline scripts", async ({
    page,
  }) => {
    const violations: string[] = [];
    page.on("console", (msg) => {
      const text = msg.text();
      if (
        text.includes("Content Security Policy") ||
        text.includes("Refused to execute inline script")
      ) {
        violations.push(text);
      }
    });

    const response = await page.goto("/sign-in");
    await page.waitForLoadState("networkidle");

    const csp = response?.headers()["content-security-policy"] ?? "";
    test.skip(
      !csp.includes("'nonce-"),
      "no nonce CSP on this origin (raw Next dev/start) — run against the Rust origin via PLAYWRIGHT_BASE_URL",
    );

    // Every inline script must carry the per-request nonce; an
    // un-nonced one is blocked under 'strict-dynamic' and whatever
    // it powers silently doesn't run (I1 was next-themes' no-flash
    // bootstrap — theme class never applied before hydration).
    const unnonced = await page.evaluate(() =>
      Array.from(document.querySelectorAll("script:not([src])"))
        .filter(
          (s) =>
            !(s as HTMLScriptElement).nonce && s.textContent!.trim().length > 0,
        )
        .map((s) => s.textContent!.slice(0, 120)),
    );
    expect(unnonced).toEqual([]);
    expect(violations).toEqual([]);
  });
});
