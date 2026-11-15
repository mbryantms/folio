/**
 * A view card is one link (WP-9.1): a click anywhere on it, the cover
 * stack included, opens the view, and a drag on the same spot arranges
 * the board without navigating.
 *
 * Both gestures start on the cover stack with real mouse input
 * (`page.mouse`), because the stack is decoration under the name link's
 * stretched `::after`: a locator click would refuse it ("another element
 * intercepts pointer events"), which is exactly the behaviour under test.
 *
 * Runs in `chromium-admin` (the shared admin session). It makes two
 * collections of its own and deletes them in `finally`; both board
 * states are axe-clean, so the a11y sweep running beside it passes
 * whichever one it meets.
 */
import { test, expect, type Locator, type Page } from "@playwright/test";

import { csrfToken, json, registrationOpen } from "./support/admin";
import { expectNoAxeViolations } from "./support/axe";

const FIRST = "E2E card link A";
const SECOND = "E2E card link B";

type Collection = { id: string; name: string };

function card(page: Page, name: string): Locator {
  return page.locator("[data-view-card]").filter({
    has: page.getByRole("link", { name, exact: true }),
  });
}

async function centre(target: Locator): Promise<{ x: number; y: number }> {
  const box = await target.boundingBox();
  expect(box, "target is laid out").toBeTruthy();
  return { x: box!.x + box!.width / 2, y: box!.y + box!.height / 2 };
}

/** The two test cards' names in the order the board draws them. */
async function boardOrder(page: Page): Promise<string[]> {
  const names = await page
    .locator("[data-view-card]")
    .getByRole("link")
    .allTextContents();
  return names.filter((n) => n === FIRST || n === SECOND);
}

test.describe("View cards", () => {
  test.beforeEach(async ({ request }) => {
    test.skip(
      !(await registrationOpen(request)),
      "needs the Rust origin with COMIC_LOCAL_REGISTRATION_OPEN=true (compose.test.yml)",
    );
  });

  test("the cover stack opens the collection; a drag arranges instead", async ({
    page,
    context,
  }) => {
    const csrf = { "X-CSRF-Token": await csrfToken(context) };
    const series = (
      await json<{ items: { id: string; name: string }[] }>(
        page.request.get("/api/series?limit=50"),
      )
    ).items.find((s) => s.name === "Relay");
    expect(series, "fixture series Relay").toBeTruthy();

    // A run that died before its `finally` leaves its collections behind.
    const stale = (
      await json<Collection[]>(page.request.get("/api/me/collections"))
    ).filter((c) => c.name === FIRST || c.name === SECOND);
    for (const collection of stale) {
      await page.request.delete(`/api/me/collections/${collection.id}`, {
        headers: csrf,
      });
    }

    const made: Collection[] = [];
    try {
      for (const name of [FIRST, SECOND]) {
        const collection = await json<Collection>(
          page.request.post("/api/me/collections", {
            headers: csrf,
            data: { name },
          }),
        );
        made.push(collection);
        // One entry each, so the stack draws a real cover.
        await json(
          page.request.post(`/api/me/collections/${collection.id}/entries`, {
            headers: csrf,
            data: { entry_kind: "series", ref_id: series!.id },
          }),
        );
      }

      await page.goto("/collections");
      const first = card(page, FIRST);
      const second = card(page, SECOND);
      await expect(first).toBeVisible();
      await expect(second).toBeVisible();

      // One link per card, named after the collection alone.
      await expect(first.getByRole("link")).toHaveCount(1);
      await expectNoAxeViolations(page, "collections board, whole-card links");

      // 1. A click on the cover stack navigates.
      const stack = first.locator(".cover-stack");
      await expect(stack).toBeVisible();
      const at = await centre(stack);
      await page.mouse.click(at.x, at.y);
      await expect(page).toHaveURL(new RegExp(`/views/${made[0]!.id}$`));

      // 2. A drag from the same spot reorders and stays on the board.
      await page.goto("/collections");
      await expect(card(page, FIRST)).toBeVisible();
      await expect(card(page, SECOND)).toBeVisible();
      const before = await boardOrder(page);
      expect(before).toHaveLength(2);
      const [lead, trail] = before as [string, string];
      const from = await centre(card(page, lead).locator(".cover-stack"));
      const to = await centre(card(page, trail));
      await page.mouse.move(from.x, from.y);
      await page.mouse.down();
      // Past the mouse sensor's 6 px, then across to the other card.
      await page.mouse.move(from.x + 12, from.y + 4, { steps: 4 });
      await page.mouse.move(to.x, to.y, { steps: 12 });
      await page.mouse.move(to.x + 4, to.y, { steps: 2 });
      await page.mouse.up();

      await expect.poll(() => boardOrder(page)).toEqual([trail, lead]);
      await expect(page).toHaveURL(/\/collections$/);
    } finally {
      for (const collection of made) {
        await page.request.delete(`/api/me/collections/${collection.id}`, {
          headers: csrf,
        });
      }
    }
  });
});
