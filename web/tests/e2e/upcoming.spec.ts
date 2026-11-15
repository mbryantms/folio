/**
 * Upcoming releases, end to end (upcoming-releases 1.0): the path a real
 * install takes, with nothing mocked.
 *
 *   admin turns both settings on → generates the ingest token → the pusher
 *   POSTs a snapshot with that token and no session → `/upcoming` shows
 *   the pull-list issue with Mylar's status → the dialog reports it → the
 *   sidebar counts it → the admin page lists what Mylar is doing.
 *
 * The smoke stack has no Metron credentials, so the release calendar is
 * empty and the page is fed by the snapshot alone: a pull-list-only row,
 * which the shared admin (who can open every library) is allowed to see.
 *
 * View only: the test also pins that the dialog offers no control beyond
 * its own Close.
 *
 * Runs in `chromium-admin` (the shared admin session). It turns the
 * feature on for its own duration and puts everything back in `finally`:
 * token revoked with the stored snapshot purged, both settings off. Both
 * states of `/upcoming` are axe-clean, so the a11y sweep running beside it
 * passes whichever one it meets.
 */
import { test, expect, type Page } from "@playwright/test";

import { csrfToken, json, registrationOpen } from "./support/admin";
import { expectNoAxeViolations } from "./support/axe";

const SERIES = "E2E Pull List Comic";
const CV_SERIES_ID = 990_001;
const CV_ISSUE_ID = 9_900_017;

/** The browser's local date: the page sends the same one as "today". */
function localDate(): string {
  const now = new Date();
  const y = now.getFullYear();
  const m = String(now.getMonth() + 1).padStart(2, "0");
  const d = String(now.getDate()).padStart(2, "0");
  return `${y}-${m}-${d}`;
}

function snapshot(today: string) {
  return {
    meta: {
      schema_version: 1,
      generated_at: new Date().toISOString(),
      mylar_db_version: 1,
      annuals_included: false,
    },
    pull_list: [
      {
        ship_date: today,
        publisher: "Folio Test Comics",
        series: SERIES,
        issue: "7",
        status: "Wanted",
        cv_series_id: CV_SERIES_ID,
        cv_issue_id: CV_ISSUE_ID,
        series_year: 2026,
      },
    ],
    watchlist: [[CV_SERIES_ID, "Active", true]],
    wanted: [
      {
        cv_series_id: CV_SERIES_ID,
        cv_issue_id: CV_ISSUE_ID,
        series: SERIES,
        issue: "7",
        store_date: today,
        status: "Wanted",
        added_at: new Date().toISOString(),
      },
    ],
    history: [],
    queue: { counts: {}, items: [] },
    jobs: [],
  };
}

async function setFlags(page: Page, on: boolean): Promise<void> {
  const res = await page.request.patch("/api/admin/settings", {
    headers: { "X-CSRF-Token": await csrfToken(page.context()) },
    data: { "upcoming.enabled": on, "integrations.mylar.enabled": on },
  });
  expect(res.status(), await res.text()).toBe(200);
}

test.describe("Upcoming releases", () => {
  test.beforeEach(async ({ request }) => {
    test.skip(
      !(await registrationOpen(request)),
      "needs the Rust origin with COMIC_LOCAL_REGISTRATION_OPEN=true (compose.test.yml)",
    );
  });

  test("a pushed snapshot shows on /upcoming, read-only and axe-clean", async ({
    page,
    playwright,
    baseURL,
  }) => {
    test.setTimeout(90_000);
    const today = localDate();
    try {
      // 1. Admin: feature on, token generated (plaintext returned once).
      await setFlags(page, true);
      const { token } = await json<{ token: string }>(
        page.request.post("/api/admin/integrations/mylar/token", {
          headers: { "X-CSRF-Token": await csrfToken(page.context()) },
        }),
      );
      expect(token).toMatch(/^fmi_/);

      // 2. The pusher: a client with no session, only the token.
      const pusher = await playwright.request.newContext({ baseURL });
      const refused = await pusher.post("/integrations/mylar/snapshot", {
        headers: { Authorization: "Bearer fmi_not-the-token" },
        data: snapshot(today),
      });
      expect(refused.status()).toBe(401);
      const pushed = await pusher.post("/integrations/mylar/snapshot", {
        headers: { Authorization: `Bearer ${token}` },
        data: snapshot(today),
      });
      expect(pushed.status(), await pushed.text()).toBe(204);
      await pusher.dispose();

      // 3. The page: this week holds the tracked, wanted issue.
      await page.goto("/upcoming");
      await expect(
        page.getByRole("heading", { level: 1, name: "Upcoming" }),
      ).toBeVisible();
      const week = page.getByRole("region", { name: "This week" });
      const card = week.locator("[data-card]").filter({ hasText: SERIES });
      await expect(card).toBeVisible();
      await expect(card.locator("[data-stage]")).toHaveText("Wanted");
      // The sidebar entry exists only while the feature is on.
      await expect(
        page.getByRole("link", { name: "Upcoming", exact: true }).first(),
      ).toBeVisible();
      // …and carries this week's count for the reader's series.
      await expect(
        page.locator('[aria-label$="out this week"]').first(),
      ).toBeVisible();
      await expectNoAxeViolations(page, "upcoming, covers");

      // 4. The dialog reports, and offers nothing but Close.
      await card.click();
      const dialog = page.getByRole("dialog");
      await expect(dialog.getByText(`${SERIES} #7`)).toBeVisible();
      await expect(dialog.getByText("Mylar is looking for it.")).toBeVisible();
      await expect(dialog.getByText("Mylar's pull list")).toBeVisible();
      await expect(dialog.getByRole("button")).toHaveCount(1);
      await expect(dialog.getByRole("button")).toHaveText("Close");
      await expectNoAxeViolations(page, "upcoming, release dialog");
      await page.keyboard.press("Escape");
      await expect(dialog).toBeHidden();

      // 5. The filter is a server param: Tracked keeps it, New #1s drops it.
      const toolbar = page.locator("[data-toolbar-row]");
      await toolbar.getByRole("button", { name: "Tracked" }).click();
      await expect(card).toBeVisible();
      await toolbar.getByRole("button", { name: "New #1s" }).click();
      await expect(
        page.getByText("No releases match these filters"),
      ).toBeVisible();
      await page.getByRole("button", { name: "Clear filters" }).click();
      await expect(card).toBeVisible();

      // 6. The list layout, then the phone composition.
      await toolbar.getByRole("radio", { name: "List" }).click();
      await expect(
        week.getByRole("button", { name: `${SERIES} #7` }),
      ).toBeVisible();
      await expectNoAxeViolations(page, "upcoming, list");
      await page.setViewportSize({ width: 375, height: 812 });
      await expect(page.locator("[data-phone-controls]")).toBeVisible();
      await expectNoAxeViolations(page, "upcoming, list, phone");

      // 7. The admin's view of the same snapshot: what Mylar is doing,
      //    with nothing to press.
      await page.setViewportSize({ width: 1280, height: 800 });
      await page.goto("/admin/upcoming");
      const activity = page
        .locator("[data-slot=card]")
        .filter({ hasText: "What Mylar is doing" });
      await expect(activity.getByText(`${SERIES} #7`)).toBeVisible();
      await expect(activity.getByRole("button")).toHaveCount(0);
      await expectNoAxeViolations(page, "admin upcoming, Mylar activity");
    } finally {
      // Put the shared stack back: no token, no stored snapshot, both off.
      const revoked = await page.request.delete(
        "/api/admin/integrations/mylar/token?purge=true",
        { headers: { "X-CSRF-Token": await csrfToken(page.context()) } },
      );
      expect([200, 204, 404]).toContain(revoked.status());
      await setFlags(page, false);
    }

    // 8. Off again: the page says so and the list is gone.
    await page.setViewportSize({ width: 1280, height: 800 });
    await page.goto("/upcoming");
    await expect(
      page.getByText("Upcoming releases are turned off"),
    ).toBeVisible();
    await expectNoAxeViolations(page, "upcoming, turned off");
  });
});
