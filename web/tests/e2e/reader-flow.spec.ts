/**
 * Reader-flow E2E against the production images (compose.test.yml booted
 * with `just docker-test`-style wiring; CI runs it in the `docker-smoke` job).
 *
 *   [setup project: register first user (auto-admin) → create a library over
 *   the generated fixture → scan] → series page → reader → ArrowRight →
 *   progress persisted (API + reloaded chrome) → offline outbox + download.
 *
 * This is the one CI gate that exercises hydration, event handlers, the
 * Rust→Next proxy hop, cookies + CSRF, the scan pipeline and the reader
 * store end to end — the layer a React / Next / Radix / TanStack bump can
 * break while every unit test stays green. Keep it short; it is not a
 * feature suite.
 *
 * Runs in the `chromium-admin` project: it starts signed in as the admin
 * that `admin.setup.ts` registered through the real sign-in form, with the
 * fixture library already scanned (WP-8.5; registering here would race the
 * other admin specs for the first-user admin role).
 *
 * Needs PLAYWRIGHT_BASE_URL pointing at the Rust origin (http://localhost:8080)
 * with COMIC_LOCAL_REGISTRATION_OPEN=true; it skips otherwise so a plain
 * `pnpm test:e2e` against raw Next still exits clean.
 */
import { test, expect } from "@playwright/test";
import { ADMIN_EMAIL, json, registrationOpen } from "./support/admin";
import { expectNoAxeViolations } from "./support/axe";

test.describe("Reader flow", () => {
  test.beforeEach(async ({ context, request }) => {
    test.skip(
      !(await registrationOpen(request)),
      "needs the Rust origin with COMIC_LOCAL_REGISTRATION_OPEN=true (compose.test.yml)",
    );
    // The reader shows a first-run overlay for fresh browsers; pre-dismiss it
    // so the spec asserts the reader, not the onboarding.
    await context.addInitScript(() => {
      window.localStorage.setItem("reader:firstRunSeen:v1", "1");
    });
  });

  test("series → reader → page turn → progress persisted", async ({
    page,
    context,
  }) => {
    test.setTimeout(180_000);

    // 1. Signed in as the shared admin (setup project).
    const me = await json<{ email: string; role: string }>(
      page.request.get("/api/auth/me"),
    );
    expect(me.email).toBe(ADMIN_EMAIL);
    expect(me.role).toBe("admin");

    // 2-3. The setup project scanned the fixture library; this spec reads
    //      the three-page "Test Series" (the others belong to the
    //      relationship spec).
    type SeriesList = { items: { slug: string; name: string }[] };
    const found = (
      await json<SeriesList>(page.request.get("/api/series?limit=50"))
    ).items.find((s) => s.name === "Test Series");
    expect(found, "fixture series Test Series").toBeTruthy();
    const series = found!;
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
    // The page image is server-rendered, so it is visible before React
    // hydrates; a key pressed before then is lost. The chrome is a lazy
    // client-only chunk (WP-4.4), so its presence proves both hydration and
    // the keymap are live.
    const readerReady = () =>
      expect(page.getByTestId("reader-chrome")).toBeAttached({
        timeout: 15_000,
      });
    await readerReady();

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
    await readerReady();
    await page.keyboard.press("t");
    await expect(
      page.getByRole("button", { name: "Page 2 of 3; click to jump" }),
    ).toBeVisible();

    // 7b. Accessibility (WP-4.8): axe-clean with the chrome shown…
    const chrome = page.getByTestId("reader-chrome");
    await expect(chrome).toHaveAttribute("data-state", "open");
    // Focus inside the header pins the 4 s auto-hide for the axe run.
    await page.getByRole("button", { name: "Exit reader" }).focus();
    await page.waitForTimeout(400); // let the 300 ms slide-in settle
    await expectNoAxeViolations(page, "reader, chrome shown");

    // …with the page-text panel open (`r`). The panel, its status region
    // and its controls are what axe inspects, so any status phase will
    // do. Don't wait for OCR to settle: a fresh smoke container downloads
    // the detector model on first use, which has held the request past
    // 90 s on CI runners (the OCR outcome is covered by the jsdom tests).
    await page.keyboard.press("r");
    const panel = page.getByRole("dialog", { name: "Page text" });
    await expect(panel).toBeVisible();
    await expect(panel.getByRole("status").first()).toHaveText(
      /Finding text|Reading text|No text detected|Couldn't detect|Couldn't read|text block/,
    );
    await expectNoAxeViolations(page, "reader, page-text panel open");
    // Esc closes the panel without also quitting the reader.
    await page.keyboard.press("Escape");
    await expect(panel).toBeHidden();
    expect(new URL(page.url()).pathname).toContain("/read/");

    // …and in the default state (chrome hidden). The first Tab stop is the
    // skip link that reveals the chrome and moves focus into it — the
    // keyboard route to the controls without knowing `t` (audit AC-2).
    await page.reload();
    await expect(page.locator("img[src*='/pages/']").first()).toBeVisible();
    await expect(chrome).toHaveAttribute("data-state", "closed");
    await expectNoAxeViolations(page, "reader, chrome hidden");
    await page.keyboard.press("Tab");
    const skip = page.getByRole("button", { name: /^Show reader controls/ });
    await expect(skip).toBeFocused();
    await page.keyboard.press("Enter");
    await expect(chrome).toHaveAttribute("data-state", "open");
    await expect(
      page.getByRole("button", { name: "Exit reader" }),
    ).toBeFocused();

    // 8. Series page now offers to continue rather than start.
    await page.goto(`/series/${series.slug}`);
    await expect(
      page.getByRole("link", { name: "Continue reading" }),
    ).toBeVisible();

    // 9. Durable progress outbox (WP-4.5): turn a page offline, kill the
    //    tab before anything reaches the server, come back online and
    //    relaunch — the queued write replays from IndexedDB.
    await page.goto(`/read/${series.slug}/${issue.slug}`);
    await expect(page.locator("img[src*='/pages/']").first()).toBeVisible();
    await readerReady();
    await context.setOffline(true);
    await page.keyboard.press("ArrowRight");
    const queued = () =>
      page.evaluate(
        () =>
          new Promise<number>((resolve) => {
            const req = indexedDB.open("folio-outbox");
            req.onerror = () => resolve(-1);
            req.onsuccess = () => {
              const db = req.result;
              if (!db.objectStoreNames.contains("entries")) {
                db.close();
                return resolve(0);
              }
              const count = db
                .transaction("entries")
                .objectStore("entries")
                .count();
              count.onsuccess = () => {
                db.close();
                resolve(count.result);
              };
            };
          }),
      );
    await expect.poll(queued, { message: "write queued offline" }).toBe(1);
    // "Killed offline": the unload flush must not get out either. Unloading
    // fires `pagehide`, whose keepalive request is handed to the browser
    // process and escapes Playwright's per-page offline emulation, so the
    // write reached the server anyway. Take the page's network away first.
    await page.evaluate(() => {
      window.fetch = () => Promise.reject(new TypeError("Failed to fetch"));
    });
    await page.close();
    const serverPage = async () =>
      (
        await json<Progress>(
          context.request.get(
            `/api/progress?issue_id=${encodeURIComponent(issue.id)}`,
          ),
        )
      ).records[0]?.page;
    await context.setOffline(false);
    expect(await serverPage(), "nothing reached the server offline").toBe(1);
    const relaunched = await context.newPage();
    await relaunched.goto("/");
    await expect
      .poll(serverPage, { message: "outbox replays on relaunch" })
      .toBe(2);

    // 10. Per-issue offline download (WP-4.6): download from the issue
    //     menu, go offline, open the reader URL (the worker redirects to
    //     the stored offline shell), read a page, reconnect — the page
    //     turn made offline replays to the server.
    const reader = relaunched;
    // The worker controls a page only after a reload (no clients.claim).
    await reader.evaluate(async () => {
      await navigator.serviceWorker.ready;
    });
    await reader.reload();
    await reader.waitForFunction(() => !!navigator.serviceWorker.controller);
    await reader.goto(`/series/${series.slug}/issues/${issue.slug}`);
    await reader.getByRole("button", { name: "Issue actions" }).click();
    await reader
      .getByRole("menuitem", { name: "Download for offline…" })
      .click();
    const dialog = reader.getByRole("dialog");
    await expect(dialog.getByText(/Estimated size/)).toBeVisible();
    await dialog.getByRole("radio", { name: /Original/ }).click();
    await dialog.getByRole("button", { name: "Download" }).click();
    const offlineState = () =>
      reader.evaluate(
        (issueId) =>
          new Promise<string>((resolve) => {
            const req = indexedDB.open("folio-offline");
            req.onerror = () => resolve("no-db");
            req.onsuccess = () => {
              const db = req.result;
              if (!db.objectStoreNames.contains("downloads")) {
                db.close();
                return resolve("no-store");
              }
              const all = db
                .transaction("downloads")
                .objectStore("downloads")
                .getAll();
              all.onsuccess = () => {
                db.close();
                const row = (
                  all.result as { issueId: string; status: string }[]
                ).find((r) => r.issueId === issueId);
                resolve(row?.status ?? "missing");
              };
            };
          }),
        issue.id,
      );
    await expect
      .poll(offlineState, { timeout: 30_000, message: "download completes" })
      .toBe("complete");
    // The offline shell is stored after the first completed download.
    await expect
      .poll(
        () =>
          reader.evaluate(async () => {
            const cache = await caches.open("folio-offline-shell-v1");
            return !!(await cache.match("/downloads"));
          }),
        { timeout: 30_000, message: "offline shell stored" },
      )
      .toBe(true);
    await expect(
      reader.getByText(/is ready to read offline/).first(),
    ).toBeVisible();

    // Settings → Downloads lists it with its size.
    await reader.goto("/settings/downloads");
    await expect(reader.getByText(/^Downloaded · /)).toBeVisible();

    const before = (
      await json<{
        records: { issue_id: string; page: number; run?: number }[];
      }>(
        context.request.get(
          `/api/progress?issue_id=${encodeURIComponent(issue.id)}`,
        ),
      )
    ).records[0]!;

    await context.setOffline(true);
    await reader.goto(`/read/${series.slug}/${issue.slug}`);
    await reader.waitForURL(/\/downloads\?from=/);
    const offlinePage = reader.locator("img[src*='/pages/']").first();
    await expect(offlinePage).toBeVisible();
    await expect
      .poll(() =>
        offlinePage.evaluate((el) => (el as HTMLImageElement).naturalWidth),
      )
      .toBe(100);
    // The issue was finished on its last page (step 9), so the offline
    // open restarts from the cover as a new run, exactly like online.
    await reader.keyboard.press("ArrowRight");
    const queuedProgress = () =>
      reader.evaluate(
        () =>
          new Promise<number>((resolve) => {
            const req = indexedDB.open("folio-outbox");
            req.onerror = () => resolve(-1);
            req.onsuccess = () => {
              const db = req.result;
              if (!db.objectStoreNames.contains("entries")) {
                db.close();
                return resolve(0);
              }
              const all = db
                .transaction("entries")
                .objectStore("entries")
                .getAll();
              all.onsuccess = () => {
                db.close();
                resolve(
                  (all.result as { kind: string }[]).filter(
                    (e) => e.kind === "progress",
                  ).length,
                );
              };
            };
          }),
      );
    await expect
      .poll(queuedProgress, { message: "offline page turn queued" })
      .toBe(1);
    await context.setOffline(false);
    await expect
      .poll(
        async () =>
          (
            await json<{
              records: { issue_id: string; page: number; run?: number }[];
            }>(
              context.request.get(
                `/api/progress?issue_id=${encodeURIComponent(issue.id)}`,
              ),
            )
          ).records[0],
        { timeout: 30_000, message: "offline progress replays on reconnect" },
      )
      .toMatchObject({ page: 1, run: (before.run ?? 0) + 1 });
    await expect.poll(queuedProgress).toBe(0);
  });
});
