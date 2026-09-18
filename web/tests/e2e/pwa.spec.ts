import { test, expect, type Page } from "@playwright/test";

async function controlled(page: Page) {
  await page.goto("/sign-in");
  const started = Date.now();
  await page.evaluate(async () => {
    await navigator.serviceWorker.register("/sw.js");
    await navigator.serviceWorker.ready;
  });
  await test.info().attach("worker-ready.json", {
    body: JSON.stringify({
      milliseconds: Date.now() - started,
      profile: test.info().project.name,
      origin: new URL(page.url()).origin,
    }),
    contentType: "application/json",
  });
  await page.reload();
  await page.waitForFunction(() => !!navigator.serviceWorker.controller);
}

test("installed worker supports a cold offline document launch", async ({
  page,
  context,
}) => {
  await controlled(page);
  await context.setOffline(true);
  const started = Date.now();
  await page.goto("/bookmarks");
  await expect(
    page.getByRole("heading", { name: "Connection unavailable" }),
  ).toBeVisible();
  await expect(
    page.getByRole("link", { name: "Try this page again" }),
  ).toHaveAttribute("href", "");
  await test.info().attach("offline-launch.json", {
    body: JSON.stringify({
      milliseconds: Date.now() - started,
      profile: test.info().project.name,
    }),
    contentType: "application/json",
  });
  await page.screenshot({
    path: test.info().outputPath("offline.png"),
    fullPage: true,
  });
  await context.setOffline(false);
  await page.getByRole("link", { name: "Open library" }).click();
  await expect(
    page.getByRole("heading", { name: "Connection unavailable" }),
  ).toHaveCount(0);
});

test("API and unknown private paths cannot use a stale cached response", async ({
  page,
  context,
}) => {
  await controlled(page);
  await page.evaluate(async () => {
    for (const name of ["apis", "others"]) {
      const cache = await caches.open(name);
      await cache.put(
        "/api/series/private-test",
        new Response("private data", { status: 200 }),
      );
      await cache.put(
        "/private-test",
        new Response("private data", { status: 200 }),
      );
    }
  });
  await context.setOffline(true);
  const leaked = await page.evaluate(async () =>
    Promise.all(
      ["/api/series/private-test", "/private-test"].map(async (url) => {
        try {
          return (await fetch(url)).ok;
        } catch {
          return false;
        }
      }),
    ),
  );
  expect(leaked).toEqual([false, false]);
});

test("worker clears private thumbnails on identity reset", async ({ page }) => {
  await controlled(page);
  const keys = await page.evaluate(async () => {
    const cache = await caches.open("folio-thumbs-v3");
    await cache.put("/issues/private/pages/0/thumb", new Response("private"));
    await new Promise<void>((resolve) => {
      const channel = new MessageChannel();
      channel.port1.onmessage = () => {
        channel.port1.close();
        resolve();
      };
      navigator.serviceWorker.controller!.postMessage(
        { type: "FOLIO_CLEAR_PRIVATE" },
        [channel.port2],
      );
    });
    return caches.keys();
  });
  expect(keys).not.toContain("folio-thumbs-v3");
});

test("manifest preserves identity and exposes app shortcuts", async ({
  request,
}) => {
  const response = await request.get("/manifest.webmanifest");
  expect(response.ok()).toBeTruthy();
  const manifest = await response.json();
  expect(manifest.id).toBe("/");
  expect(
    manifest.shortcuts.map((shortcut: { url: string }) => shortcut.url),
  ).toContain("/bookmarks");
});

test("accepting an update reloads only that window", async ({
  page,
  context,
}) => {
  await controlled(page);
  const other = await context.newPage();
  await other.goto("/sign-in");
  // Give the actual root updater a chance to bind after hydration.
  await other.waitForLoadState("networkidle");
  await page.waitForLoadState("networkidle");
  await other.evaluate(() => {
    (window as unknown as { pwaSentinel: string }).pwaSentinel =
      "still reading";
  });
  // A different script URL creates a real waiting update at the same scope,
  // without modifying files or deploying a second build during the test.
  await page.evaluate(async () => {
    await navigator.serviceWorker.register("/sw.js?update-test=1", {
      scope: "/",
    });
  });
  await expect(
    page.getByText("A new version of Folio is available."),
  ).toBeVisible();
  await page.getByRole("button", { name: "Later", exact: true }).click();
  // Sonner retains dismissed toasts during their exit animation. Reopen while
  // that node still exists, but only interact with the new active notification.
  await expect(
    page.locator('[data-sonner-toast][data-removed="true"]'),
  ).toHaveCount(1);
  await page.evaluate(() =>
    window.dispatchEvent(new Event("folio:check-update")),
  );
  const reloadButton = page
    .locator('[data-sonner-toast][data-removed="false"]')
    .getByRole("button", { name: "Reload", exact: true });
  await expect(reloadButton).toHaveCount(1);
  const reloaded = page.waitForEvent("load");
  await reloadButton.click();
  await reloaded;
  expect(
    await other.evaluate(
      () => (window as unknown as { pwaSentinel: string }).pwaSentinel,
    ),
  ).toBe("still reading");
  await other.bringToFront();
  await expect(
    other.getByText("A new version of Folio is available."),
  ).toHaveCount(1);
  await expect(
    other.getByText("A new version of Folio is available."),
  ).toBeVisible();
  await other.close();
});
