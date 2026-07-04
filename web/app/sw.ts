/// <reference no-default-lib="true" />
/// <reference lib="esnext" />
/// <reference lib="webworker" />

/**
 * Service worker entry. Compiled to `public/sw.js` by `@serwist/cli`
 * in a separate post-`next build` step (see `serwist.config.js` and
 * the `sw:compile` package script — NOT `@serwist/next`, whose
 * Webpack requirement blows the reader-bundle budget; see
 * `next.config.ts`). Registration is manual, via `@serwist/window`
 * in `components/ServiceWorkerLoader.tsx`.
 *
 * Scope of caching:
 *
 * - **Precache the Next.js app shell** (HTML + RSC payloads + JS
 *   chunks + CSS). This is what makes the app launch when offline
 *   — the chrome appears immediately, then any data-dependent
 *   surface degrades gracefully when the network call fails.
 * - **Runtime-cache static assets** with cache-first: images
 *   under `/icons/*`, the manifest, the web app's own static
 *   bundles.
 * - **DO NOT cache authenticated API responses.** Folio's API
 *   surface is per-user and per-permission. A stale cache of
 *   `/series` from one user's session would leak into another's
 *   on the same device. Always-network for everything under
 *   `/series`, `/issues`, `/me`, `/admin`, `/auth`, `/opds`,
 *   `/libraries`, `/views`, `/collections`, `/markers`, `/log`,
 *   `/rails`, `/audit`, `/search`, `/me/cbl-lists`,
 *   `/health-issues`, `/scan-runs`, `/scan-preview`, `/removed`,
 *   `/folder-tree`, `/filter-options`, `/me/sessions`,
 *   `/me/app-passwords`, `/me/rail-dismissals`, `/me/pages`,
 *   `/me/views`, `/me/progress`, `/me/preferences`,
 *   `/me/account`, `/me/email`, `/me/sync`, and similar.
 * - **Runtime-cache cover/strip thumbnails** (`/issues/{id}/pages/
 *   {n}/thumb` + `/issues/{id}/covers/{id}`) with a bounded
 *   stale-while-revalidate cache (audit FEP-3) — covers survive
 *   offline launches and HTTP-cache eviction.
 * - **DO NOT cache page bytes.** Comic page bytes (`/issues/{id}/
 *   pages/{n}`) are large, per-user-authorised, and the offline-
 *   reading feature that would intentionally cache them gets its
 *   own driver (see PWA hardening notes Tier 2 follow-up).
 * - **DO NOT cache WebSocket upgrades** (`/ws/*`). The SW
 *   transparently passes them through.
 *
 * If a fetch is for a same-origin path that doesn't match any of
 * the runtime cache rules, the SW falls through to the network
 * with no caching. That preserves the live behavior for every API
 * route — the SW is purely additive, never a stale layer.
 */
import { defaultCache } from "@serwist/next/worker";
import type { PrecacheEntry, SerwistGlobalConfig } from "serwist";
import { ExpirationPlugin, Serwist, StaleWhileRevalidate } from "serwist";

// Serwist injects the precache manifest onto the SW global at
// build time. Without the declaration, TypeScript can't see it.
// The augmentation is intentionally on `ServiceWorkerGlobalScope`
// (not the broader `WorkerGlobalScope`) so it does not leak into
// other web-worker entry points like `web/workers/decode.ts`,
// whose `DedicatedWorkerGlobalScope` would otherwise inherit
// these properties and break unrelated typechecks.
declare global {
  interface ServiceWorkerGlobalScope extends SerwistGlobalConfig {
    __SW_MANIFEST: (PrecacheEntry | string)[] | undefined;
  }
}

declare const self: ServiceWorkerGlobalScope;

/** Pre-generated cover/strip thumbnails + applied provider covers —
 *  the only `/issues/…` byte surfaces the SW may cache (audit FEP-3).
 *  Full-resolution page bytes (`/pages/{n}` without `/thumb`) stay
 *  uncached by design. */
const THUMB_PATH_RE = /^\/issues\/[^/]+\/(?:pages\/\d+\/thumb|covers\/[^/]+)$/;

/** Runtime cache for thumbnails. `-v2` retires the original
 *  `folio-thumbs`, whose entries could be poisoned by pre-v0.26.2
 *  immutable HTTP-cache responses (purged on activate below). */
const THUMB_CACHE = "folio-thumbs-v2";

const serwist = new Serwist({
  precacheEntries: self.__SW_MANIFEST,
  // `skipWaiting: false` so the new SW does not steal control
  // until the user actively reloads (or accepts the update toast
  // from `useServiceWorkerUpdate`). Without this gate, a deploy
  // mid-read would silently swap the bundle on the next nav.
  skipWaiting: false,
  // `clientsClaim: false` — never seize a page that loaded *without*
  // the SW. On WebKit (iOS Safari + installed PWA), claiming an
  // already-open client mid-session left that page's first
  // client-side RSC navigation hanging forever — the "fresh tab →
  // first pill click stuck on the loading skeleton; a reload fixes
  // it" report. A reloaded page loads already-controlled and is
  // consistent; an uncontrolled page stays uncontrolled and runs its
  // navigations straight against the network. Either is fine — the
  // mid-session hand-off was the only broken state, so we remove it.
  clientsClaim: false,
  // `navigationPreload: false` — every navigation / RSC request is
  // handed to the native loader in the `fetch` listener below, so
  // serwist never consumes `event.preloadResponse`. An
  // enabled-but-unconsumed navigation preload *itself* stalls
  // navigations on WebKit, so it has to be off once navigations
  // bypass serwist.
  navigationPreload: false,
  // The default cache set covers Next.js's static assets, font
  // requests, and image responses with sensible strategies. The
  // explicit list of API-route exclusions is enforced by the
  // path-pattern guards below taking precedence over the
  // catch-all default.
  //
  // The thumbnail rule sits AHEAD of the defaults (audit FEP-3):
  // cover/strip thumbs are extensionless URLs under `/issues/`, so
  // the extension-matching `static-image-assets` default never
  // caught them and the `/issues/` guard below force-bypassed them —
  // no offline covers, and repeat browsing paid a re-fetch whenever
  // the HTTP cache had evicted. Stale-while-revalidate (not
  // cache-first) because an archive page-edit regenerates a thumb
  // under the SAME URL — SWR self-heals on the next view instead of
  // pinning the old art until eviction. Cache keys are the full URL
  // including `?variant=` / `?v=`. Bounded: covers a few screens of
  // grid browsing without letting a 24k-issue library eat the
  // origin's storage quota.
  runtimeCaching: [
    {
      matcher: ({ sameOrigin, url }) =>
        sameOrigin && THUMB_PATH_RE.test(url.pathname),
      handler: new StaleWhileRevalidate({
        cacheName: THUMB_CACHE,
        // `no-cache` forces the revalidation fetch to be validated
        // against the ORIGIN (conditional request; 304 keeps it
        // cheap). Left at the default, fetch() is answered by the
        // browser's HTTP cache — and thumb responses cached under the
        // pre-v0.26.2 `public, max-age=1y, immutable` policy answer
        // without ever touching the network, so a post-edit
        // regeneration could never reach this cache: SWR re-cached
        // the same stale bytes forever.
        fetchOptions: { cache: "no-cache" },
        plugins: [
          new ExpirationPlugin({
            maxEntries: 1200,
            maxAgeSeconds: 30 * 24 * 60 * 60,
            maxAgeFrom: "last-used",
          }),
        ],
      }),
    },
    ...defaultCache,
  ],
});

// One-time purge of the pre-v2 thumb cache. Its entries were
// revalidated through the poisoned HTTP cache (see `fetchOptions`
// above), so any thumbnail viewed before the fix may be permanently
// wrong in it — dropping the whole cache is the only reliable reset.
// Runs on activate, i.e. once per SW update; deleting a missing cache
// is a no-op.
self.addEventListener("activate", (event) => {
  event.waitUntil(caches.delete("folio-thumbs"));
});

// Hard guard: any same-origin request to a backend API surface
// (and the app-route HTML/RSC navigations that share these path
// shapes, e.g. `/series/…`) is never cached and never touched by
// the SW. The list mirrors the documented "DO NOT cache" set
// above; the `fetch` listener below hands every match to the
// browser's native loader.
//
// Two kinds of entries live here:
//   1. Backend API surfaces (per-user, per-permission JSON) — never
//      cache, per the doc comment above.
//   2. App-route HTML/RSC navigation *destinations* reached by
//      client-side `<Link>` clicks / `router.push`. These MUST be
//      handed to the native loader so serwist's `defaultCache` never
//      re-issues their RSC fetch via `respondWith` — that forwards
//      the App Router's abort signal and, when a navigation is
//      superseded, rejects and strands the router on `loading.tsx`
//      (the "reader-exit hang" documented in the `fetch` listener).
//      Every entity/detail route that isn't part of the offline
//      app-shell belongs here: `/creators/`, `/read/`, `/settings/`,
//      `/bookmarks`, `/pages/`, alongside `/series/`, `/libraries/`,
//      `/views/`, `/collections/`. The library root (`/`) is left out
//      on purpose — it's the precached shell that launches offline.
const API_PATH_PREFIXES = [
  "/admin/",
  "/audit",
  "/auth/",
  "/bookmarks",
  "/collections/",
  "/creators/",
  "/filter-options",
  "/folder-tree",
  "/health-issues",
  "/issues/",
  "/libraries/",
  "/log",
  "/markers/",
  "/me/",
  "/opds/",
  "/pages/",
  "/rails/",
  "/read/",
  "/removed",
  "/scan-preview",
  "/scan-runs",
  "/search",
  "/series/",
  "/settings/",
  "/views/",
  "/ws/",
];

self.addEventListener("fetch", (event: FetchEvent) => {
  const url = new URL(event.request.url);
  if (url.origin !== self.location.origin) {
    // Leave cross-origin requests (provider cover CDNs like
    // static.metron.cloud / comicvine.gamespot.com) entirely to the
    // browser's native loader. A bare `return` here is NOT enough:
    // serwist's `defaultCache` registers its own fetch listener via
    // `addEventListeners()` below, and its cross-origin rule would
    // intercept + re-fetch these as opaque no-cors responses — which
    // are incompatible with this document's `COEP: credentialless`,
    // so Firefox blocks the image with NS_ERROR_INTERCEPTION_FAILED
    // (candidate covers render blank). `stopImmediatePropagation`
    // prevents serwist's listener from running, so no `respondWith`
    // is called and the browser fetches natively — credentialless,
    // no CORP required.
    event.stopImmediatePropagation();
    return;
  }
  // Hand EVERY App Router navigation + RSC fetch/prefetch to the native
  // loader, regardless of path. A full-page load is `mode: "navigate"`;
  // a client-side `<Link>` click / `router.push` / viewport prefetch
  // carries the `RSC` header. serwist must never `respondWith` these —
  // it re-issues the request carrying the router's abort signal, and
  // when a navigation is superseded the response rejects and strands
  // the router (hung loading skeleton, then dead links). This is
  // path-agnostic, so no future route can out-run the per-path
  // allowlist below — that list now only matters for non-RSC
  // same-origin API GETs that happen to share an app-route path shape.
  if (
    event.request.mode === "navigate" ||
    event.request.headers.get("RSC") === "1"
  ) {
    event.stopImmediatePropagation();
    return;
  }
  const path = url.pathname;
  // Thumbnail/cover bytes are the one `/issues/…` surface serwist DOES
  // handle (the SWR cache above) — skip the bypass guard for them
  // (audit FEP-3). Everything else under these prefixes stays
  // native-loader-only.
  if (THUMB_PATH_RE.test(path)) {
    return;
  }
  if (
    API_PATH_PREFIXES.some(
      (p) => path === p.replace(/\/$/, "") || path.startsWith(p),
    )
  ) {
    // Same as the cross-origin branch: hand the request to the browser's
    // native loader, do NOT re-issue it via `respondWith(fetch(...))`.
    //
    // The old `event.respondWith(fetch(event.request))` re-fetched the
    // request *carrying its original abort signal*. App-route paths like
    // `/series/{slug}/issues/{slug}` are not just JSON API calls — they're
    // also the destination of client-side RSC navigations (e.g. the reader
    // exit button, `router.push(exitUrl)`). When the App Router aborts or
    // supersedes an in-flight RSC fetch, the forwarded signal aborted our
    // re-fetch too, `respondWith` rejected, and the router got stuck on the
    // route's `loading.tsx` until a hard reload — the reader-exit hang.
    // `stopImmediatePropagation` keeps serwist's caching listener from
    // running (so the "never cache API paths" guarantee still holds) while
    // letting the browser perform the request itself, signal intact.
    event.stopImmediatePropagation();
  }
});

serwist.addEventListeners();
