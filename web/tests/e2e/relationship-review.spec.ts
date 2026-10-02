/**
 * Relationship-suggestion review smoke (WP-7.3): open the admin review
 * queue, accept the top pending suggestion with one click, and check the
 * relationship pair landed (API + the series page's Related block).
 *
 * **Needs an existing admin and at least one suggestion**, so it is opt-in:
 *
 *   E2E_ADMIN_EMAIL=… E2E_ADMIN_PASSWORD=… \
 *   PLAYWRIGHT_BASE_URL=http://localhost:8080 pnpm exec playwright test relationship-review
 *
 * It signs in with those credentials (never registers: in the docker-smoke
 * job `reader-flow.spec.ts` must be the first — admin — registration, and a
 * second registering spec would race it for the role). With no pending
 * suggestion it queues an engine run and polls briefly; if there is still
 * none it skips. The docker-smoke fixture library is one series, so the
 * engine has nothing to suggest there and the spec skips cleanly in CI.
 * It really accepts a suggestion — point it at a disposable instance.
 */
import { test, expect, type APIRequestContext } from "@playwright/test";

const EMAIL = process.env.E2E_ADMIN_EMAIL;
const PASSWORD = process.env.E2E_ADMIN_PASSWORD;

type Series = { id: string; slug: string; name: string };
type Suggestion = {
  id: string;
  from_series: Series;
  to_series: Series;
  kind: string;
  kind_label: string;
  status: string;
};
type ListView = { items: Suggestion[] };

async function firstPending(
  request: APIRequestContext,
): Promise<Suggestion | undefined> {
  const res = await request.get(
    "/api/admin/relationship-suggestions?status=pending&limit=1",
  );
  expect(res.ok(), `list → ${res.status()}`).toBeTruthy();
  return ((await res.json()) as ListView).items[0];
}

test.describe("Relationship review", () => {
  test.skip(
    !EMAIL || !PASSWORD,
    "set E2E_ADMIN_EMAIL / E2E_ADMIN_PASSWORD (an existing admin) to run",
  );

  test("accept a suggestion from the review queue", async ({
    page,
    context,
  }) => {
    test.setTimeout(90_000);

    // 1. Sign in through the JSON login (CSRF-exempt; sets the session +
    //    CSRF cookies on the shared browser context).
    const login = await page.request.post("/auth/local/login", {
      data: { email: EMAIL, password: PASSWORD },
    });
    expect(login.status(), await login.text()).toBe(200);
    const me = (await (await page.request.get("/api/auth/me")).json()) as {
      role: string;
    };
    test.skip(me.role !== "admin", "E2E_ADMIN_EMAIL is not an admin");
    const csrf = (await context.cookies()).find(
      (c) => c.name === "__Host-comic_csrf",
    )?.value;
    expect(csrf, "CSRF cookie").toBeTruthy();

    // 2. Make sure there is something to review.
    let target = await firstPending(page.request);
    if (!target) {
      const run = await page.request.post(
        "/api/admin/relationship-suggestions/run",
        { headers: { "X-CSRF-Token": csrf! } },
      );
      expect(run.status()).toBe(202);
      await expect
        .poll(async () => (target = await firstPending(page.request)), {
          timeout: 20_000,
        })
        .toBeTruthy()
        .catch(() => undefined);
    }
    test.skip(!target, "no pending relationship suggestions to review");
    const s = target!;

    // 3. The review queue lists it first (highest confidence); accept it.
    await page.goto("/admin/relationships");
    await expect(
      page.getByRole("heading", { name: "Relationships" }),
    ).toBeVisible();
    const row = page.locator(`[data-suggestion-id="${s.id}"]`);
    await expect(row).toBeVisible();
    await expect(row.getByText(s.kind_label).first()).toBeVisible();
    const accepted = page.waitForResponse(
      (r) =>
        r
          .url()
          .endsWith(`/api/admin/relationship-suggestions/${s.id}/accept`) &&
        r.request().method() === "POST",
    );
    await row.getByRole("button", { name: "Accept", exact: true }).click();
    const res = await accepted;
    expect(res.status(), await res.text()).toBe(200);
    // It leaves the pending queue.
    await expect(row).toHaveCount(0);

    // 4. The pair exists (created via create_pair, source = suggested)…
    const rels = (await (
      await page.request.get(`/api/series/${s.from_series.slug}/relationships`)
    ).json()) as {
      relationships: { series: Series; source: string; kind: string }[];
    };
    expect(
      rels.relationships.some(
        (r) => r.series.id === s.to_series.id && r.source === "suggested",
      ),
    ).toBeTruthy();

    // 5. …and shows on the series page's Related block.
    await page.goto(`/series/${s.from_series.slug}`);
    const related = page.locator(
      "section[aria-labelledby='series-related-heading']",
    );
    await expect(related).toBeVisible();
    await expect(related.getByText(s.to_series.name).first()).toBeVisible();
  });
});
