/**
 * Reader-flow E2E against the production images (compose.test.yml booted
 * with `just docker-test`-style wiring; CI runs it in the `docker-smoke` job).
 *
 *   register first user (auto-admin) → create a library over the generated
 *   fixture (web/tests/e2e/fixtures/make-library.mjs) → scan → series page
 *   → reader → ArrowRight → progress persisted (API + reloaded chrome).
 *
 * This is the one CI gate that exercises hydration, event handlers, the
 * Rust→Next proxy hop, cookies + CSRF, the scan pipeline and the reader
 * store end to end — the layer a React / Next / Radix / TanStack bump can
 * break while every unit test stays green. Keep it short; it is not a
 * feature suite.
 *
 * Needs PLAYWRIGHT_BASE_URL pointing at the Rust origin (http://localhost:8080)
 * with COMIC_LOCAL_REGISTRATION_OPEN=true; it skips otherwise so a plain
 * `pnpm test:e2e` against raw Next still exits clean.
 */
import {
  test,
  expect,
  type APIRequestContext,
  type BrowserContext,
} from "@playwright/test";

const PASSWORD = "correct-horse-battery-staple";

async function csrfToken(context: BrowserContext): Promise<string> {
  const cookie = (await context.cookies()).find(
    (c) => c.name === "__Host-comic_csrf",
  );
  expect(cookie, "CSRF cookie set after registration").toBeTruthy();
  return cookie!.value;
}

async function json<T>(
  req: Promise<Awaited<ReturnType<APIRequestContext["get"]>>>,
): Promise<T> {
  const res = await req;
  expect(res.ok(), `${res.url()} → ${res.status()}`).toBeTruthy();
  return (await res.json()) as T;
}

test.describe("Reader flow", () => {
  test.beforeEach(async ({ context, request }) => {
    const cfg = await request.get("/api/auth/config");
    const open =
      cfg.ok() &&
      ((await cfg.json()) as { registration_open?: boolean }).registration_open;
    test.skip(
      !open,
      "needs the Rust origin with COMIC_LOCAL_REGISTRATION_OPEN=true (compose.test.yml)",
    );
    // The reader shows a first-run overlay for fresh browsers; pre-dismiss it
    // so the spec asserts the reader, not the onboarding.
    await context.addInitScript(() => {
      window.localStorage.setItem("reader:firstRunSeen:v1", "1");
    });
  });

  test("register → library → scan → reader → page turn → progress persisted", async ({
    page,
    context,
  }) => {
    test.setTimeout(180_000);

    // 1. Register the first user through the real form (becomes admin and is
    //    signed in on the 201 response).
    const email = `e2e-${Date.now()}@example.test`;
    await page.goto("/sign-in");
    await expect(
      page.getByRole("heading", { name: "Sign in to Folio" }),
    ).toBeVisible();
    await page.getByRole("tab", { name: "Register" }).click();
    await page.getByLabel(/^Email$/).fill(email);
    await page.getByLabel(/^Password$/).fill(PASSWORD);
    await page.getByRole("button", { name: "Create account" }).click();
    await page.waitForURL((u) => u.pathname === "/");
    const me = await json<{ email: string; role: string }>(
      page.request.get("/api/auth/me"),
    );
    expect(me.email).toBe(email);
    expect(me.role).toBe("admin");

    // 2. Create the library over /library and scan it (same endpoint the
    //    admin "New library" dialog posts to; cookie session + CSRF header).
    const csrf = await csrfToken(context);
    const created = await page.request.post("/api/libraries", {
      headers: { "X-CSRF-Token": csrf },
      data: { name: "E2E Library", root_path: "/library", scan_now: true },
    });
    expect(created.status(), await created.text()).toBe(201);

    // 3. The scan is async (apalis worker); the series appearing is the
    //    outcome we care about.
    type SeriesList = { items: { slug: string; name: string }[] };
    await expect
      .poll(
        async () =>
          (await json<SeriesList>(page.request.get("/api/series"))).items
            .length,
        { timeout: 90_000, message: "scan should produce one series" },
      )
      .toBeGreaterThan(0);
    const series = (await json<SeriesList>(page.request.get("/api/series")))
      .items[0];
    expect(series.name).toBe("Test Series");
    type IssueList = {
      items: { id: string; slug: string; page_count?: number | null }[];
    };
    const issue = (
      await json<IssueList>(
        page.request.get(`/api/series/${series.slug}/issues`),
      )
    ).items[0];
    expect(issue, "scan should produce one issue").toBeTruthy();

    // 4. Series page CTA → reader (a real client-side navigation).
    await page.goto(`/series/${series.slug}`);
    const read = page.getByRole("link", { name: /^Read$/ });
    await expect(read).toBeVisible();
    await read.click();
    await page.waitForURL(new RegExp(`/read/${series.slug}/${issue.slug}`));
    // The first page image is served by the Rust origin and must decode.
    const firstPage = page.locator("img[src*='/pages/']").first();
    await expect(firstPage).toBeVisible();
    await expect
      .poll(() =>
        firstPage.evaluate((el) => (el as HTMLImageElement).naturalWidth),
      )
      .toBe(100);

    // 5. Turn one page; the debounced progress write must reach the server.
    const progressWrite = page.waitForResponse(
      (r) =>
        r.url().endsWith("/api/progress") &&
        r.request().method() === "POST" &&
        r.ok(),
    );
    await page.keyboard.press("ArrowRight");
    await progressWrite;

    // 6. Persisted server-side (0-based page index).
    type Progress = { records: { issue_id: string; page: number }[] };
    const progress = await json<Progress>(
      page.request.get(
        `/api/progress?issue_id=${encodeURIComponent(issue.id)}`,
      ),
    );
    expect(progress.records[0]?.page).toBe(1);

    // 7. …and the reader resumes there: reload, reveal the chrome (`t`) and
    //    read the page counter.
    await page.reload();
    await expect(page.locator("img[src*='/pages/']").first()).toBeVisible();
    await page.keyboard.press("t");
    await expect(
      page.getByRole("button", { name: "Page 2 of 3; click to jump" }),
    ).toBeVisible();

    // 8. Series page now offers to continue rather than start.
    await page.goto(`/series/${series.slug}`);
    await expect(
      page.getByRole("link", { name: "Continue reading" }),
    ).toBeVisible();
  });
});
