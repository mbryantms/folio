/**
 * WP-4.6 service-worker routes: offline navigations boot the stored
 * reader shell (or redirect to it), downloaded bytes are served only to
 * the shell document and only from the owning account's cache, and the
 * shell precache crawls and prunes hashed assets.
 */
import "fake-indexeddb/auto";
import { beforeEach, describe, expect, it, vi } from "vitest";

import {
  indexedOfflineDb,
  offlineCacheName,
  OFFLINE_SHELL_CACHE,
  recordKey,
  META_ACCOUNT,
  type DownloadRecord,
} from "@/lib/pwa/offline-store";
import { extractAssetRefs, warmOfflineShell } from "@/lib/pwa/offline-shell";

import { asCacheStorage, FakeCacheStorage } from "./fake-caches";

const mock = vi.hoisted(() => ({ precache: vi.fn() }));
vi.mock("serwist", () => ({
  Serwist: class {
    addEventListeners() {}
    matchPrecache = mock.precache;
  },
  CacheFirst: class {},
  ExpirationPlugin: class {},
}));

let listeners: Record<string, (event: never) => void>;
let caches: FakeCacheStorage;

function request(
  path: string,
  options: { mode?: string; referrer?: string } = {},
) {
  const event = {
    request: {
      url: `https://folio.test${path}`,
      method: "GET",
      mode: options.mode ?? "no-cors",
      referrer: options.referrer ?? "",
      headers: new Headers(),
    },
    stopImmediatePropagation: vi.fn(),
    respondWith: vi.fn(),
    waitUntil: vi.fn(),
  };
  listeners.fetch!(event as never);
  return event;
}

const response = async (event: ReturnType<typeof request>) =>
  (await event.respondWith.mock.calls[0]![0]) as Response;

function record(account: string, issueId: string, status = "complete") {
  return {
    key: recordKey(account, issueId),
    account,
    issueId,
    seriesId: "s",
    seriesSlug: "series",
    issueSlug: `issue-${issueId}`,
    status,
    pages: [],
    pageCount: 1,
  } as unknown as DownloadRecord;
}

beforeEach(async () => {
  vi.resetModules();
  vi.clearAllMocks();
  listeners = {};
  caches = new FakeCacheStorage();
  // The worker opens the default database name; start each test clean.
  await indexedOfflineDb().clear();
  vi.stubGlobal("self", {
    location: { origin: "https://folio.test" },
    __SW_MANIFEST: [],
    addEventListener: (name: string, cb: (event: never) => void) => {
      listeners[name] = cb;
    },
  });
  vi.stubGlobal("caches", asCacheStorage(caches));
  vi.stubGlobal("fetch", vi.fn().mockRejectedValue(new TypeError("offline")));
  mock.precache.mockResolvedValue(new Response("offline page"));
  await import("@/app/sw");
});

async function seedDownload(account = "alice", issueId = "i1") {
  const db = indexedOfflineDb();
  await db.setMeta(META_ACCOUNT, account);
  await db.update(recordKey(account, issueId), () => record(account, issueId));
  const cache = await caches.open(offlineCacheName(account));
  await cache.put(
    `/issues/${issueId}/pages/0`,
    new Response("page bytes", { headers: { "content-type": "image/webp" } }),
  );
}

async function seedShell() {
  const shell = await caches.open(OFFLINE_SHELL_CACHE);
  await shell.put("/downloads", new Response("shell document"));
}

describe("offline navigation", () => {
  it("serves the stored shell for /downloads", async () => {
    await seedShell();
    const res = await response(
      request("/downloads?issue=i1", { mode: "navigate" }),
    );
    expect(await res.text()).toBe("shell document");
  });

  it("redirects other paths to the shell when a download is complete", async () => {
    await seedShell();
    await seedDownload();
    const res = await response(
      request("/read/series/issue-i1", { mode: "navigate" }),
    );
    expect(res.status).toBe(302);
    expect(res.headers.get("location")).toBe(
      "https://folio.test/downloads?from=%2Fread%2Fseries%2Fissue-i1",
    );
  });

  it("keeps the public offline page when nothing is downloaded", async () => {
    await seedShell();
    const res = await response(request("/series/x", { mode: "navigate" }));
    expect(await res.text()).toBe("offline page");
  });

  it("uses the network whenever it answers", async () => {
    vi.mocked(fetch).mockResolvedValue(new Response("live", { status: 200 }));
    await seedShell();
    const res = await response(request("/downloads", { mode: "navigate" }));
    expect(await res.text()).toBe("live");
  });
});

describe("downloaded bytes", () => {
  it("serves any variant of a downloaded page to the offline shell", async () => {
    await seedDownload();
    const event = request("/issues/i1/pages/0?w=720&v=abc", {
      referrer: "https://folio.test/downloads?issue=i1",
    });
    expect(event.stopImmediatePropagation).toHaveBeenCalled();
    expect(await (await response(event)).text()).toBe("page bytes");
    expect(fetch).not.toHaveBeenCalled();
  });

  it("leaves page bytes to the native loader for every other document", () => {
    const event = request("/issues/i1/pages/0", {
      referrer: "https://folio.test/read/series/issue-i1",
    });
    expect(event.stopImmediatePropagation).toHaveBeenCalled();
    expect(event.respondWith).not.toHaveBeenCalled();
  });

  it("never serves another account's cache", async () => {
    await seedDownload("alice");
    // Ownership moved to bob (alice's purge not yet run).
    await indexedOfflineDb().setMeta(META_ACCOUNT, "bob");
    vi.mocked(fetch).mockResolvedValue(
      new Response("network", { status: 404 }),
    );
    const res = await response(
      request("/issues/i1/pages/0", {
        referrer: "https://folio.test/downloads",
      }),
    );
    expect(await res.text()).toBe("network");
  });

  it("falls through to the network for pages that are not downloaded", async () => {
    await seedDownload();
    vi.mocked(fetch).mockResolvedValue(new Response("fresh"));
    const res = await response(
      request("/issues/i9/pages/0", {
        referrer: "https://folio.test/downloads",
      }),
    );
    expect(await res.text()).toBe("fresh");
  });
});

describe("shell precache", () => {
  it("finds hashed assets in HTML, flight data, chunks and stylesheets", () => {
    const html =
      '<script src="/_next/static/chunks/main-abc.js"></script>' +
      '<link rel="stylesheet" href="/_next/static/css/app-1.css">' +
      '<script>self.__next_f.push([1,"static/chunks/page-9f.js"])</script>';
    expect(extractAssetRefs(html).sort()).toEqual([
      "/_next/static/chunks/main-abc.js",
      "/_next/static/chunks/page-9f.js",
      "/_next/static/css/app-1.css",
    ]);
    expect(
      extractAssetRefs(
        "@font-face{src:url(../media/font.abc.woff2)}",
        "/_next/static/css/app-1.css",
      ),
    ).toEqual(["/_next/static/media/font.abc.woff2"]);
  });

  it("stores the credential-less document last, crawls lazy chunks, prunes stale ones", async () => {
    const bodies: Record<string, string> = {
      "/downloads": '<script src="/_next/static/chunks/a.js"></script>',
      "/_next/static/chunks/a.js": 'import("static/chunks/lazy-b.js")',
      "/_next/static/chunks/lazy-b.js": "done",
    };
    const doFetch = vi.fn(async (path: string, init?: RequestInit) => {
      if (path === "/downloads") expect(init?.credentials).toBe("omit");
      return new Response(bodies[path] ?? "", {
        status: bodies[path] ? 200 : 404,
        headers: {
          "content-type":
            path === "/downloads" ? "text/html" : "text/javascript",
        },
      });
    });
    const shell = await caches.open(OFFLINE_SHELL_CACHE);
    await shell.put("/_next/static/chunks/old.js", new Response("old"));
    const result = await warmOfflineShell({
      fetch: doFetch as unknown as typeof fetch,
      caches: asCacheStorage(caches),
    });
    expect(result).toEqual({ assets: 2, html: true });
    expect([...shell.entries.keys()].sort()).toEqual([
      "/_next/static/chunks/a.js",
      "/_next/static/chunks/lazy-b.js",
      "/downloads",
    ]);
  });
});
