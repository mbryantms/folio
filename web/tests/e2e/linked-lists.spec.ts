/**
 * Linked reading lists: a master list with an event linked after its
 * first issue. The master's page draws the event as a band at its place,
 * and finishing the anchor issue offers the event on the end-of-issue
 * card beside the main line; starting it opens the event in the reader,
 * remembering the master (`?via=`).
 *
 * Runs in `chromium-admin` (the shared admin session). The master reads
 * "Two Step" (no other spec reads it) and the event holds Relay (2016)
 * #1. Both lists are uploaded fresh and deleted in `finally`, and the
 * progress the reader writes is reset there too.
 */
import { test, expect, type Page } from "@playwright/test";

import { csrfToken, json, registrationOpen } from "./support/admin";

const MASTER = "E2E linked master";
const EVENT = "E2E linked event";

const cbl = (name: string, books: string) =>
  `<?xml version="1.0" encoding="utf-8"?>
<ReadingList><Name>${name}</Name><Books>
${books}
</Books></ReadingList>
`;

const MASTER_CBL = cbl(
  MASTER,
  `<Book Series="Two Step" Number="1" Volume="2018" Year="2018" />
<Book Series="Two Step" Number="2" Volume="2018" Year="2018" />`,
);
const EVENT_CBL = cbl(
  EVENT,
  `<Book Series="Relay" Number="1" Volume="2016" Year="2016" />`,
);

type Entry = {
  id: string;
  position: number;
  matched_issue_id?: string | null;
  issue?: { slug: string; series_slug: string } | null;
};

async function upload(
  page: Page,
  csrf: Record<string, string>,
  xml: string,
  name: string,
) {
  return json<{ id: string }>(
    page.request.post("/api/me/cbl-lists/upload", {
      headers: csrf,
      multipart: {
        file: {
          name: `${name}.cbl`,
          mimeType: "application/xml",
          buffer: Buffer.from(xml, "utf8"),
        },
        name,
      },
    }),
  );
}

/** The list's entries once every one has matched an issue. */
async function matchedEntries(page: Page, listId: string): Promise<Entry[]> {
  let entries: Entry[] = [];
  await expect
    .poll(
      async () => {
        entries = (
          await json<{ items: Entry[] }>(
            page.request.get(`/api/me/cbl-lists/${listId}/entries`),
          )
        ).items;
        return (
          entries.length > 0 &&
          entries.every((e) => e.matched_issue_id && e.issue)
        );
      },
      { message: `${listId} entries match the fixture`, timeout: 20_000 },
    )
    .toBe(true);
  return entries;
}

test.describe("Linked reading lists", () => {
  test.beforeEach(async ({ context, request }) => {
    test.skip(
      !(await registrationOpen(request)),
      "needs the Rust origin with COMIC_LOCAL_REGISTRATION_OPEN=true (compose.test.yml)",
    );
    await context.addInitScript(() => {
      window.localStorage.setItem("reader:firstRunSeen:v1", "1");
    });
  });

  test("an event shows in its master and is offered at its place", async ({
    page,
    context,
  }) => {
    test.setTimeout(120_000);
    const csrf = { "X-CSRF-Token": await csrfToken(context) };

    // A run that died before its `finally` leaves its lists behind.
    type ListRow = { id: string; parsed_name: string };
    const stale = (
      await json<{ items: ListRow[] }>(page.request.get("/api/me/cbl-lists"))
    ).items.filter((l) => l.parsed_name === MASTER || l.parsed_name === EVENT);
    for (const l of stale) {
      await page.request.delete(`/api/me/cbl-lists/${l.id}`, { headers: csrf });
    }

    const made: string[] = [];
    const read: string[] = [];
    try {
      const master = await upload(page, csrf, MASTER_CBL, MASTER);
      made.push(master.id);
      const event = await upload(page, csrf, EVENT_CBL, EVENT);
      made.push(event.id);
      const masterEntries = await matchedEntries(page, master.id);
      const [eventEntry] = await matchedEntries(page, event.id);
      const anchor = masterEntries.find((e) => e.position === 0)!;
      read.push(anchor.matched_issue_id!, eventEntry!.matched_issue_id!);

      // Both issues start unread: the event is offered only while it has
      // something unread, and the anchor must open on its first page.
      for (const issueId of read) {
        await json(
          page.request.post("/api/progress", {
            headers: csrf,
            data: { issue_id: issueId, page: 0, finished: false },
          }),
        );
      }

      const view = await json<{ id: string }>(
        page.request.post("/api/me/saved-views", {
          headers: csrf,
          data: { kind: "cbl", name: MASTER, cbl_list_id: master.id },
        }),
      );
      await json(
        page.request.post(`/api/me/cbl-lists/${master.id}/links`, {
          headers: csrf,
          data: {
            child_list_id: event.id,
            kind: "event",
            after_entry_id: anchor.id,
          },
        }),
      );

      // 1. The master draws the event as a band.
      await page.goto(`/views/${view.id}`);
      const band = page.locator("[data-event-link]");
      await expect(band).toHaveCount(1);
      await expect(band).toContainText(EVENT);

      // 2. Reading the anchor issue in the master's context, the end card
      //    offers the event beside the main line.
      const issue = anchor.issue!;
      await page.goto(
        `/read/${issue.series_slug}/${issue.slug}?cbl=${encodeURIComponent(view.id)}`,
      );
      await expect(page.getByTestId("reader-chrome")).toBeAttached({
        timeout: 15_000,
      });
      const card = page.getByRole("dialog", { name: "End of issue" });
      const start = card.getByRole("button", {
        name: new RegExp(`Start ${EVENT}`),
      });
      await expect(async () => {
        await page.keyboard.press("ArrowRight");
        await expect(start).toBeVisible({ timeout: 2_000 });
      }).toPass({ timeout: 20_000 });

      // 3. Starting it reads the event, remembering the master.
      await start.click();
      await page.waitForURL(
        (url) =>
          url.pathname.startsWith("/read/") &&
          url.searchParams.get("via") === view.id &&
          url.searchParams.get("cbl") !== view.id,
      );
    } finally {
      for (const issueId of read) {
        await page.request.post("/api/progress", {
          headers: csrf,
          data: { issue_id: issueId, page: 0, finished: false },
        });
      }
      for (const id of made) {
        await page.request.delete(`/api/me/cbl-lists/${id}`, { headers: csrf });
      }
    }
  });
});
