/**
 * Relationship review + the M7b surfaces' accessibility (WP-7.3, WP-8.5).
 *
 * Runs in the docker-smoke job, in the `chromium-admin` project: it starts
 * signed in as the admin `admin.setup.ts` registered, with the fixture
 * library scanned. The fixture (`fixtures/make-library.mjs`) holds two
 * volumes of one title — "Relay (2011)" (Volume 1) and "Relay (2016)"
 * (Volume 2) — so the scan's post-scan relationship-suggestion run has a
 * name-continuation proposal: "Relay (2016) continues Relay (2011)".
 *
 *   wait for the pending suggestion (async job; bounded poll) → Related tab
 *   with the suggestion chip (axe) → /admin/relationships pending list
 *   (axe) → "Edit kind" popover with the kind picker open (axe) → Accept →
 *   the pair exists (API) → Related tab with the reading-order strip and
 *   the Similar rail (axe) → "Add relationship" dialog with the kind picker
 *   expanded (axe).
 *
 * Axe uses the shared WCAG 2.2 AA assertion (`support/axe.ts`); any
 * violation fails, whatever its impact.
 */
import { test, expect, type APIRequestContext } from "@playwright/test";

import { json, registrationOpen } from "./support/admin";
import { expectNoAxeViolations } from "./support/axe";

type Series = { id: string; slug: string; name: string };
type Suggestion = {
  id: string;
  from_series: Series;
  to_series?: Series | null;
  kind: string;
  kind_label: string;
  status: string;
};
type ListView = { items: Suggestion[] };

/** The fixture's continuation suggestion with `status`, if it exists. */
async function relaySuggestion(
  request: APIRequestContext,
  status: "pending" | "accepted",
): Promise<Suggestion | undefined> {
  const list = await json<ListView>(
    request.get(
      `/api/admin/relationship-suggestions?status=${status}&limit=200`,
    ),
  );
  return list.items.find(
    (s) =>
      s.kind === "continues" &&
      s.from_series.name === "Relay" &&
      s.to_series?.name === "Relay",
  );
}

test.describe("Relationship review", () => {
  test.beforeEach(async ({ request }) => {
    test.skip(
      !(await registrationOpen(request)),
      "needs the Rust origin with COMIC_LOCAL_REGISTRATION_OPEN=true (compose.test.yml)",
    );
  });

  test("accept a suggestion; the M7b surfaces are axe-clean", async ({
    page,
  }, testInfo) => {
    test.setTimeout(150_000);

    // 1. The post-scan suggestion job is async: poll for its proposal. On a
    //    CI retry after the accept already went through, pick up from the
    //    accepted row instead (the queue part ran on the first attempt).
    let pending: Suggestion | undefined;
    let accepted: Suggestion | undefined;
    await expect
      .poll(
        async () => {
          pending = await relaySuggestion(page.request, "pending");
          if (!pending && testInfo.retry > 0) {
            accepted = await relaySuggestion(page.request, "accepted");
          }
          return Boolean(pending ?? accepted);
        },
        {
          timeout: 90_000,
          message: "post-scan run proposes Relay (2016) continues Relay (2011)",
        },
      )
      .toBe(true);
    const s = (pending ?? accepted)!;
    const from = s.from_series;
    const to = s.to_series!;
    expect(from.id).not.toBe(to.id);

    // Visible only: right after a navigation the DOM can briefly hold a
    // second, hidden copy of the panel while the page streams in.
    const relatedTab = page
      .getByTestId("series-related-tab")
      .filter({ visible: true });
    const related = relatedTab.locator(
      "section[aria-labelledby='series-related-heading']",
    );

    if (pending) {
      // 2. Related tab before review: the admin suggestion chip.
      await page.goto(`/series/${from.slug}?tab=related`);
      await expect(relatedTab).toBeVisible();
      await expect(
        related.getByRole("heading", { name: "Related series" }),
      ).toBeVisible();
      await expect(related.getByText(/^Continues$/).first()).toBeVisible();
      await expectNoAxeViolations(page, "series Related tab, pending chip");

      // 3. The review queue lists it; axe on the pending list.
      await page.goto("/admin/relationships");
      await expect(
        page.getByRole("heading", { name: "Relationships" }),
      ).toBeVisible();
      const row = page.locator(`[data-suggestion-id="${s.id}"]`);
      await expect(row).toBeVisible();
      await expect(row.getByText(s.kind_label).first()).toBeVisible();
      await expectNoAxeViolations(page, "/admin/relationships pending list");

      // 4. "Edit kind" (accept as another kind): popover + picker open.
      await row.getByRole("button", { name: "Edit kind" }).click();
      const kindPicker = page.getByRole("combobox", {
        name: "Relationship kind",
      });
      await expect(kindPicker).toBeVisible();
      await kindPicker.click();
      await expect(page.getByRole("listbox")).toBeVisible();
      await expectNoAxeViolations(page, "/admin/relationships edit kind");
      // Close the picker, then the popover, without accepting.
      await page.keyboard.press("Escape");
      await expect(page.getByRole("listbox")).toBeHidden();
      await page.keyboard.press("Escape");
      await expect(kindPicker).toBeHidden();

      // 5. Accept as suggested.
      const acceptRes = page.waitForResponse(
        (r) =>
          r
            .url()
            .endsWith(`/api/admin/relationship-suggestions/${s.id}/accept`) &&
          r.request().method() === "POST",
      );
      await row.getByRole("button", { name: "Accept", exact: true }).click();
      const res = await acceptRes;
      expect(res.status(), await res.text()).toBe(200);
      // It leaves the pending queue.
      await expect(row).toHaveCount(0);
    }

    // 6. The pair exists (created via create_pair, source = suggested).
    const rels = await json<{
      relationships: { series: Series; source: string; kind: string }[];
      chain: { series: Series }[];
    }>(page.request.get(`/api/series/${from.slug}/relationships`));
    expect(
      rels.relationships.some(
        (r) =>
          r.series.id === to.id &&
          r.kind === "continues" &&
          r.source === "suggested",
      ),
    ).toBeTruthy();
    expect(rels.chain.map((c) => c.series.id)).toEqual([to.id, from.id]);

    // 7. The Related tab with relationships: grouped link, reading-order
    //    strip and the Similar rail (an accepted relationship alone lists
    //    the other volume as similar).
    await page.goto(`/series/${from.slug}?tab=related`);
    await expect(relatedTab).toBeVisible();
    await expect(
      related.getByRole("list", { name: "Reading order" }),
    ).toBeVisible();
    await expect(
      related.locator("[data-testid='related-card']").first(),
    ).toBeVisible();
    const similar = relatedTab.locator(
      "section[aria-labelledby='similar-series-heading']",
    );
    await expect(similar).toBeVisible();
    await expect(similar.getByText("Relay").first()).toBeVisible();
    await expectNoAxeViolations(page, "series Related tab with relationships");

    // 8. "Add relationship" dialog with the kind picker expanded.
    await related.getByRole("button", { name: "Add relationship" }).click();
    const dialog = page.getByRole("dialog");
    await expect(dialog).toBeVisible();
    const picker = dialog.getByRole("combobox", { name: "Relationship" });
    await picker.click();
    await expect(picker).toHaveAttribute("aria-expanded", "true");
    await expect(page.getByRole("listbox")).toBeVisible();
    await expectNoAxeViolations(
      page,
      "add-relationship dialog, kind picker open",
    );
  });
});
