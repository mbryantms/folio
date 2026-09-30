/// <reference no-default-lib="true" />
/// <reference lib="esnext" />
/// <reference lib="webworker" />

import { CacheFirst, ExpirationPlugin, Serwist } from "serwist";
import type { PrecacheEntry, SerwistGlobalConfig } from "serwist";
import {
  isPublicAsset,
  LEGACY_CACHES,
  THUMB_CACHE,
  THUMB_PATH,
} from "../lib/pwa/cache-policy";
import {
  META_ACCOUNT,
  OFFLINE_SHELL_CACHE,
  OFFLINE_SHELL_PATH,
  indexedOfflineDb,
  matchOfflineAsset,
  offlineKeyFor,
} from "../lib/pwa/offline-store";

declare global {
  interface ServiceWorkerGlobalScope extends SerwistGlobalConfig {
    __SW_MANIFEST: (PrecacheEntry | string)[] | undefined;
  }
}
declare const self: ServiceWorkerGlobalScope;

// No authenticated HTML, RSC or JSON is cached. Offline navigation renders
// the public offline-reader shell when the user has downloads (WP-4.6),
// otherwise a public fallback — never an authenticated document. Page
// bytes are cached only as explicit per-account downloads and served only
// to the offline shell.
const serwist = new Serwist({
  precacheEntries: self.__SW_MANIFEST,
  skipWaiting: false,
  clientsClaim: false,
  navigationPreload: false,
  runtimeCaching: [
    {
      matcher: ({ sameOrigin, url }) =>
        sameOrigin && isPublicAsset(url.pathname),
      handler: new CacheFirst({
        cacheName: "folio-static-v1",
        plugins: [
          new ExpirationPlugin({ maxEntries: 300, maxAgeSeconds: 30 * 86400 }),
          // The offline shell keeps its own copy of every hashed asset it
          // needs (lib/pwa/offline-shell.ts); use it when the runtime copy
          // was evicted and the network is unreachable.
          {
            handlerDidError: async ({ request }) =>
              caches.match(request, {
                cacheName: OFFLINE_SHELL_CACHE,
                ignoreSearch: true,
              }),
          },
        ],
      }),
    },
  ],
});

const offlineDb = indexedOfflineDb();

/** Whether the device owner has at least one complete download. */
async function hasCompleteDownload(): Promise<boolean> {
  const account = await offlineDb.getMeta<string>(META_ACCOUNT);
  if (!account) return false;
  return (await offlineDb.all()).some(
    (r) => r.account === account && r.status === "complete",
  );
}

/**
 * Navigation without a network: the stored offline-reader shell when the
 * user has downloads (any other path redirects there, carrying the path
 * so `/read/<series>/<issue>` opens the downloaded issue), else the public
 * offline page.
 */
async function offlineNavigation(url: URL): Promise<Response> {
  const shell = await Promise.resolve()
    .then(() =>
      caches.match(OFFLINE_SHELL_PATH, {
        cacheName: OFFLINE_SHELL_CACHE,
        ignoreSearch: true,
        ignoreVary: true,
      }),
    )
    .catch(() => undefined);
  if (shell) {
    if (url.pathname === OFFLINE_SHELL_PATH) return shell;
    if (await hasCompleteDownload().catch(() => false)) {
      const target = new URL(OFFLINE_SHELL_PATH, self.location.origin);
      target.searchParams.set("from", url.pathname);
      return Response.redirect(target.href, 302);
    }
  }
  return (await serwist.matchPrecache("/offline.html")) ?? Response.error();
}

/** Requests made by the offline shell document (same-origin referrer). */
function fromOfflineShell(request: Request): boolean {
  try {
    const referrer = new URL(request.referrer);
    return (
      referrer.origin === self.location.origin &&
      referrer.pathname === OFFLINE_SHELL_PATH
    );
  } catch {
    return false;
  }
}

let identityEpoch = 0;
// Serialize writes and clears so a response started before logout cannot
// repopulate the cache after its deletion.
let writes: Promise<unknown> = Promise.resolve();
self.addEventListener("message", (event) => {
  if (event.data?.type !== "FOLIO_CLEAR_PRIVATE") return;
  identityEpoch++;
  writes = writes.catch(() => undefined).then(() => caches.delete(THUMB_CACHE));
  event.waitUntil(
    writes.then(() => event.ports[0]?.postMessage({ cleared: true })),
  );
});
// Durable outbox (WP-4.5): Background Sync only wakes open windows. The
// replay itself stays in the page, which holds the CSRF cookie token and
// the access-token refresh path; a worker-side POST could do neither.
self.addEventListener("sync", (event: Event) => {
  const sync = event as ExtendableEvent & { tag?: string };
  if (sync.tag !== "folio-outbox") return;
  sync.waitUntil(
    self.clients
      .matchAll({ type: "window" })
      .then((windows) =>
        windows.forEach((client) =>
          client.postMessage({ type: "FOLIO_OUTBOX_REPLAY" }),
        ),
      ),
  );
});
self.addEventListener("activate", (event) => {
  event.waitUntil(
    Promise.all(LEGACY_CACHES.map((name) => caches.delete(name))),
  );
});

self.addEventListener("fetch", (event: FetchEvent) => {
  const request = event.request;
  const url = new URL(request.url);
  const bypass = () => event.stopImmediatePropagation();
  if (
    url.origin !== self.location.origin ||
    request.method !== "GET" ||
    url.pathname === "/api" ||
    url.pathname.startsWith("/api/") ||
    request.headers.get("RSC") === "1"
  ) {
    bypass();
    return;
  }
  if (request.mode === "navigate") {
    bypass();
    event.respondWith(fetch(request).catch(() => offlineNavigation(url)));
    return;
  }
  // Downloaded pages/thumbnails, only for the offline shell: served from
  // the owning account's cache (any `?w=`/`?v=` variant maps to the stored
  // tier), else the network. Every other document keeps the native loader.
  if (offlineKeyFor(url) && fromOfflineShell(request)) {
    bypass();
    event.respondWith(
      matchOfflineAsset(offlineDb, caches, url)
        .catch(() => undefined)
        .then((hit) => hit ?? fetch(request)),
    );
    return;
  }
  if (
    THUMB_PATH.test(url.pathname) &&
    url.searchParams.get("sw") !== "bypass"
  ) {
    bypass();
    const epoch = identityEpoch;
    const cached = caches
      .open(THUMB_CACHE)
      .then(async (cache) => {
        const response = await cache.match(request);
        const date = response?.headers.get("date");
        if (
          response &&
          date &&
          Date.now() - Date.parse(date) > 30 * 86400_000
        ) {
          await cache.delete(request);
          return undefined;
        }
        return epoch === identityEpoch ? response : undefined;
      })
      .catch(() => undefined);
    const fresh = fetch(request, { cache: "no-cache" });
    event.waitUntil(
      fresh
        .then((response) => {
          if (!response.ok) return;
          const copy = response.clone();
          writes = writes
            .catch(() => undefined)
            .then(async () => {
              if (epoch !== identityEpoch) return;
              const cache = await caches.open(THUMB_CACHE);
              await cache.put(request, copy);
              const keys = await cache.keys();
              await Promise.all(
                keys
                  .slice(0, Math.max(0, keys.length - 1200))
                  .map((key) => cache.delete(key)),
              );
            });
          return writes;
        })
        .catch(() => undefined),
    );
    event.respondWith(cached.then((response) => response ?? fresh));
    return;
  }
  if (!isPublicAsset(url.pathname) && url.pathname !== "/offline.html")
    bypass();
});
serwist.addEventListeners();
