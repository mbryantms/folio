/**
 * WP-4.6 download manager: tiered page downloads into the per-account
 * cache, resume/pause, quota and network failures, series enqueue across
 * cursor pages, account scoping, eviction, and the offline resume fold.
 */
import "fake-indexeddb/auto";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { IssueDetailView, PageInfo } from "@/lib/api/types";
import {
  createDownloadManager,
  defaultDownloadTier,
  downloadPageUrl,
  estimateIssueBytes,
  foldProgress,
  resolveOfflineResume,
  trackOutboxProgress,
  type DownloadManager,
} from "@/lib/pwa/downloads";
import {
  indexedOfflineDb,
  matchOfflineAsset,
  offlineCacheName,
  offlineKeyFor,
  OFFLINE_SHELL_CACHE,
  type OfflineDb,
} from "@/lib/pwa/offline-store";
import { createOutbox, memoryStorage } from "@/lib/pwa/outbox";

import { asCacheStorage, FakeCacheStorage } from "./fake-caches";

const PNG = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 1, 2, 3, 4]);
const image = () =>
  new Response(PNG, { headers: { "content-type": "image/webp" } });

function pages(n: number, width = 2000): PageInfo[] {
  return Array.from({ length: n }, (_, i) => ({
    image: i,
    image_width: width,
    image_height: Math.round(width * 1.5),
    image_size: 900_000,
  }));
}

function issue(id: string, n = 3, width = 2000): IssueDetailView {
  return {
    id,
    slug: `issue-${id}`,
    series_id: "s1",
    series_slug: "series-one",
    library_id: "lib",
    state: "active",
    page_count: n,
    pages: pages(n, width),
    file_size: n * 900_000,
    number: "1",
    title: null,
    last_rewrite_at: null,
    manga: null,
    created_at: "",
    updated_at: "",
    file_path: "/x.cbz",
    additional_links: [],
    comic_info_raw: null,
    user_pinned_columns: [],
  } as unknown as IssueDetailView;
}

let dbCounter = 0;
let db: OfflineDb;
let caches: FakeCacheStorage;
let fetched: string[];

function manager(
  opts: {
    fetch?: (url: string, init?: RequestInit) => Promise<Response>;
    api?: (path: string) => Promise<Response>;
    concurrency?: number;
    online?: () => boolean;
  } = {},
): DownloadManager {
  return createDownloadManager({
    db,
    caches: asCacheStorage(caches),
    fetch: (async (url: string, init?: RequestInit) => {
      fetched.push(url);
      return (opts.fetch ?? (async () => image()))(url, init);
    }) as typeof fetch,
    api:
      opts.api ??
      (async (path: string) =>
        path.startsWith("/progress")
          ? new Response(JSON.stringify({ records: [] }))
          : new Response("{}", { status: 404 })),
    pageConcurrency: opts.concurrency ?? 1,
    online: opts.online ?? (() => true),
    storage: {
      persisted: vi.fn().mockResolvedValue(false),
      persist: vi.fn().mockResolvedValue(true),
      estimate: vi.fn().mockResolvedValue({ usage: 10, quota: 1000 }),
    } as unknown as StorageManager,
  });
}

async function settled(m: DownloadManager, issueId: string) {
  await vi.waitFor(() => {
    const status = m.get(issueId)?.status;
    expect(["complete", "error", "paused"]).toContain(status);
  });
  return m.get(issueId)!;
}

beforeEach(() => {
  db = indexedOfflineDb(`folio-offline-test-${dbCounter++}`);
  caches = new FakeCacheStorage();
  fetched = [];
});

describe("issue download", () => {
  it("stores every page at the chosen tier under canonical keys", async () => {
    const m = manager();
    await m.setAccount("alice");
    await m.downloadIssue(issue("i1"), 1080);
    const record = await settled(m, "i1");
    expect(record.status).toBe("complete");
    expect(record.donePages).toBe(3);
    expect(record.bytes).toBeGreaterThan(0);
    // Tier URL fetched, canonical key stored.
    expect(fetched).toContain("/issues/i1/pages/0?w=1080");
    const cache = caches.stores.get(offlineCacheName("alice"))!;
    expect([...cache.entries.keys()].sort()).toEqual(
      [
        "/issues/i1/pages/0",
        "/issues/i1/pages/1",
        "/issues/i1/pages/2",
        "/issues/i1/pages/0/thumb?variant=strip",
        "/issues/i1/pages/1/thumb?variant=strip",
        "/issues/i1/pages/2/thumb?variant=strip",
        "/issues/i1/pages/0/thumb",
      ].sort(),
    );
  });

  it("never upscales: pages narrower than the tier fetch the original", () => {
    expect(downloadPageUrl("i", 0, 1080, 800, null)).toBe("/issues/i/pages/0");
    expect(downloadPageUrl("i", 0, 1080, 2000, "v1")).toBe(
      "/issues/i/pages/0?w=1080&v=v1",
    );
    expect(downloadPageUrl("i", 0, "original", 2000, null)).toBe(
      "/issues/i/pages/0",
    );
  });

  it("does not advertise an incomplete download: strip thumbs are optional, pages are not", async () => {
    const m = manager({
      fetch: async (url) =>
        url.includes("/thumb")
          ? new Response("nope", { status: 404 })
          : url.includes("/pages/1")
            ? new Response("nope", { status: 500 })
            : image(),
    });
    await m.setAccount("alice");
    await m.downloadIssue(issue("i1"), 720);
    const record = await settled(m, "i1");
    expect(record.status).toBe("error");
    expect(record.error).toBe("network");
    expect(await m.verify("i1")).toBe(false);
  });

  it("rejects non-image bodies for pages", async () => {
    const m = manager({
      fetch: async () =>
        new Response("<html>", { headers: { "content-type": "text/html" } }),
    });
    await m.setAccount("alice");
    await m.downloadIssue(issue("i1"), 720);
    expect((await settled(m, "i1")).status).toBe("error");
  });

  it("pauses and resumes without refetching stored pages", async () => {
    let release!: () => void;
    const gate = new Promise<void>((r) => (release = r));
    const m = manager({
      fetch: async (url, init) => {
        if (url.startsWith("/issues/i1/pages/2?")) {
          await new Promise<void>((resolve, reject) => {
            init?.signal?.addEventListener("abort", () =>
              reject(new DOMException("Aborted", "AbortError")),
            );
            void gate.then(resolve);
          });
        }
        return image();
      },
    });
    await m.setAccount("alice");
    await m.downloadIssue(issue("i1"), 720);
    await vi.waitFor(() => expect(m.get("i1")?.donePages).toBe(2));
    await m.pause("i1");
    expect((await settled(m, "i1")).status).toBe("paused");
    release();
    fetched = [];
    await m.resume("i1");
    const record = await settled(m, "i1");
    expect(record.status).toBe("complete");
    expect(fetched.filter((u) => !u.includes("thumb"))).toEqual([
      "/issues/i1/pages/2?w=720",
    ]);
  });

  it("re-queues an interrupted download on the next launch", async () => {
    const first = manager({ fetch: () => new Promise(() => {}) });
    await first.setAccount("alice");
    await first.downloadIssue(issue("i1"), 720);
    await vi.waitFor(() => expect(first.get("i1")?.status).toBe("downloading"));
    // Relaunch: a fresh manager over the same database resumes it.
    const second = manager();
    await second.setAccount("alice");
    expect((await settled(second, "i1")).status).toBe("complete");
  });

  it("stops the queue on a quota error and says so", async () => {
    let online = false;
    const m = manager({ online: () => online });
    await m.setAccount("alice");
    const cache = await caches.open(offlineCacheName("alice"));
    cache.failPut = new DOMException("full", "QuotaExceededError");
    // Both queued while offline; the queue starts when back online.
    await m.downloadIssue(issue("i1"), 720);
    await m.downloadIssue(issue("i2"), 720);
    online = true;
    await m.resume("i1");
    const record = await settled(m, "i1");
    expect(record.error).toBe("quota");
    expect(record.errorMessage).toMatch(/storage is full/i);
    // The next issue is not attempted while storage is full.
    expect(m.get("i2")?.status).toBe("queued");
    expect(fetched.some((u) => u.includes("/i2/"))).toBe(false);
  });

  it("asks for persistent storage on the first download", async () => {
    const persist = vi.fn().mockResolvedValue(true);
    const m = createDownloadManager({
      db,
      caches: asCacheStorage(caches),
      fetch: (async () => image()) as typeof fetch,
      api: async () => new Response(JSON.stringify({ records: [] })),
      online: () => true,
      storage: {
        persisted: vi.fn().mockResolvedValue(false),
        persist,
      } as unknown as StorageManager,
    });
    await m.setAccount("alice");
    await m.downloadIssue(issue("i1"), 720);
    await settled(m, "i1");
    expect(persist).toHaveBeenCalledTimes(1);
  });

  it("verify demotes a complete download whose pages were evicted", async () => {
    const m = manager();
    await m.setAccount("alice");
    await m.downloadIssue(issue("i1"), 720);
    await settled(m, "i1");
    expect(await m.verify("i1")).toBe(true);
    await caches.stores
      .get(offlineCacheName("alice"))!
      .delete("/issues/i1/pages/1");
    expect(await m.verify("i1")).toBe(false);
    expect(m.get("i1")?.status).toBe("error");
  });
});

describe("series download", () => {
  it("walks every cursor page and fetches each issue's detail on its turn", async () => {
    const api = vi.fn(async (path: string) => {
      if (path.startsWith("/series/s1/issues?")) {
        const cursor = new URLSearchParams(path.split("?")[1]).get("cursor");
        const body = cursor
          ? { items: [summary("b")], next_cursor: null }
          : {
              items: [summary("a"), summary("x", "removed")],
              next_cursor: "c2",
            };
        return new Response(JSON.stringify(body));
      }
      const detail = /\/series\/series-one\/issues\/issue-(\w+)$/.exec(path);
      if (detail) return new Response(JSON.stringify(issue(detail[1]!, 2)));
      return new Response(JSON.stringify({ records: [] }));
    });
    const m = manager({ api });
    await m.setAccount("alice");
    const queued = await m.downloadSeries(
      { id: "s1", slug: "series-one", name: "Series One" },
      720,
    );
    expect(queued).toBe(2);
    expect((await settled(m, "a")).status).toBe("complete");
    expect((await settled(m, "b")).status).toBe("complete");
    expect(m.get("a")?.pages).toHaveLength(2);
    expect(m.get("x")).toBeUndefined();
    expect(api.mock.calls.map(([p]) => p)).toContain(
      "/series/s1/issues?limit=100&cursor=c2",
    );
  });
});

function summary(id: string, state = "active") {
  return {
    id,
    slug: `issue-${id}`,
    series_id: "s1",
    series_slug: "series-one",
    series_name: "Series One",
    page_count: 2,
    state,
    created_at: "",
    updated_at: "",
  };
}

describe("account scoping and eviction", () => {
  it("a different signed-in account purges the previous account's downloads", async () => {
    const alice = manager();
    await alice.setAccount("alice");
    await alice.downloadIssue(issue("i1"), 720);
    await settled(alice, "i1");
    const url = new URL("https://folio.test/issues/i1/pages/0?w=720");
    expect(
      await matchOfflineAsset(db, asCacheStorage(caches), url),
    ).toBeDefined();

    const bob = manager();
    await bob.setAccount("bob");
    expect(bob.snapshot()).toHaveLength(0);
    expect(caches.stores.has(offlineCacheName("alice"))).toBe(false);
    expect(await db.all()).toHaveLength(0);
    // The worker's lookup is scoped to the owning account.
    expect(
      await matchOfflineAsset(db, asCacheStorage(caches), url),
    ).toBeUndefined();
  });

  it("the same account keeps its downloads across sessions", async () => {
    const first = manager();
    await first.setAccount("alice");
    await first.downloadIssue(issue("i1"), 720);
    await settled(first, "i1");
    const offline = manager();
    await offline.hydrate(); // public shell: no session, same device owner
    expect(offline.account()).toBe("alice");
    expect(offline.get("i1")?.status).toBe("complete");
  });

  it("remove, removeAll and clearAll delete records and cached bytes", async () => {
    const m = manager();
    await m.setAccount("alice");
    await m.downloadIssue(issue("i1"), 720);
    await m.downloadIssue(issue("i2"), 720);
    await settled(m, "i2");
    await m.remove("i1");
    const cache = caches.stores.get(offlineCacheName("alice"))!;
    expect([...cache.entries.keys()].some((k) => k.includes("/i1/"))).toBe(
      false,
    );
    expect(m.snapshot().map((r) => r.issueId)).toEqual(["i2"]);

    await caches.open(OFFLINE_SHELL_CACHE);
    await m.removeAll();
    expect(m.snapshot()).toHaveLength(0);
    expect(caches.stores.has(offlineCacheName("alice"))).toBe(false);
    expect(caches.stores.has(OFFLINE_SHELL_CACHE)).toBe(false);

    await m.downloadIssue(issue("i3"), 720);
    await settled(m, "i3");
    await m.clearAll();
    expect(await db.all()).toHaveLength(0);
    expect(await caches.keys()).toEqual([]);
    expect(m.account()).toBeNull();
  });

  it("refuses to download without a signed-in account", async () => {
    const m = manager();
    await expect(m.downloadIssue(issue("i1"), 720)).rejects.toThrow(/sign in/i);
  });
});

describe("helpers", () => {
  it("maps any variant, version or retry URL to the stored key", () => {
    const key = (u: string) => offlineKeyFor(new URL(u, "https://folio.test"));
    expect(key("/issues/a/pages/3?w=720&v=x&r=1")).toBe("/issues/a/pages/3");
    expect(key("/issues/a/pages/3/thumb?variant=strip&v=x")).toBe(
      "/issues/a/pages/3/thumb?variant=strip",
    );
    expect(key("/issues/a/pages/0/thumb?variant=cover_small")).toBe(
      "/issues/a/pages/0/thumb",
    );
    expect(key("/issues/a/pages/3/thumb")).toBeNull();
    expect(key("/api/issues/a")).toBeNull();
  });

  it("estimates by tier and never above the original", () => {
    const p = pages(10, 2000);
    const small = estimateIssueBytes(p, 720);
    const large = estimateIssueBytes(p, 1600);
    const original = estimateIssueBytes(p, "original");
    expect(small).toBeLessThan(large);
    expect(large).toBeLessThanOrEqual(original);
    // Narrower than the tier → the original bytes are what gets stored.
    expect(estimateIssueBytes(pages(10, 600), 1080)).toBe(
      estimateIssueBytes(pages(10, 600), "original"),
    );
  });

  it("defaults the tier to the device's fit-width need", () => {
    expect(defaultDownloadTier({ width: 390, height: 844 }, 2)).toBe(1080);
    expect(defaultDownloadTier({ width: 390, height: 844 }, 3)).toBe(1600);
    expect(defaultDownloadTier({ width: 1280, height: 800 }, 1)).toBe(1080);
    expect(defaultDownloadTier({ width: 1024, height: 1366 }, 2)).toBe(
      "original",
    );
  });

  it("folds progress with the run rules", () => {
    const base = { page: 5, finished: false, run: 1 };
    expect(foldProgress(base, { issue_id: "i", page: 3, run: 1 })).toEqual(
      base,
    );
    expect(foldProgress(base, { issue_id: "i", page: 8, run: 1 }).page).toBe(8);
    expect(foldProgress(base, { issue_id: "i", page: 9, run: 0 })).toEqual(
      base,
    );
    expect(
      foldProgress(base, { issue_id: "i", page: 0, run: 1, restart: true }),
    ).toEqual({ page: 0, finished: false, run: 2 });
    expect(
      foldProgress(base, { issue_id: "i", page: 5, run: 1, finished: true })
        .finished,
    ).toBe(true);
  });

  it("resumes offline from queued writes and restarts a finished issue", () => {
    const record = {
      progress: { page: 1, finished: false, run: 0 },
      pageCount: 10,
      pages: [],
    };
    expect(
      resolveOfflineResume(record, [{ issue_id: "i", page: 6, run: 0 }], null),
    ).toEqual({ initialPage: 6, initialRun: 0, restartRun: false });
    expect(
      resolveOfflineResume(
        { ...record, progress: { page: 9, finished: true, run: 0 } },
        [],
        null,
      ),
    ).toEqual({ initialPage: 0, initialRun: 0, restartRun: true });
    // The server's newer run wins over the snapshot taken at download.
    expect(
      resolveOfflineResume(record, [], { issue_id: "i", page: 2, run: 3 }),
    ).toEqual({ initialPage: 2, initialRun: 3, restartRun: false });
  });

  it("tracks queued progress into the downloaded issue's resume position", async () => {
    const m = manager();
    await m.setAccount("alice");
    await m.downloadIssue(issue("i1"), 720);
    await settled(m, "i1");
    const outbox = createOutbox({ storage: memoryStorage() });
    const stop = trackOutboxProgress(m, outbox);
    await outbox.enqueue("progress", "i1:0", {
      issue_id: "i1",
      page: 2,
      run: 0,
    });
    await outbox.enqueue("progress", "other:0", { issue_id: "other", page: 2 });
    await vi.waitFor(() =>
      expect(m.get("i1")?.progress).toEqual({
        page: 2,
        finished: false,
        run: 0,
      }),
    );
    stop();
  });
});
