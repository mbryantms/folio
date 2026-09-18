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

declare global {
  interface ServiceWorkerGlobalScope extends SerwistGlobalConfig {
    __SW_MANIFEST: (PrecacheEntry | string)[] | undefined;
  }
}
declare const self: ServiceWorkerGlobalScope;

// No authenticated HTML, RSC, JSON, or page bytes are cached. Offline
// navigation renders a public fallback, not an authenticated app shell.
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
        ],
      }),
    },
  ],
});

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
    event.respondWith(
      fetch(request).catch(
        async () =>
          (await serwist.matchPrecache("/offline.html")) ?? Response.error(),
      ),
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
