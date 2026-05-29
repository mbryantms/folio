/// <reference no-default-lib="true" />
/// <reference lib="esnext" />
/// <reference lib="webworker" />

/**
 * Service worker entry. Compiled by @serwist/next at build time
 * (see `next.config.ts::withSerwistInit`) and written to
 * `public/sw.js`. The compiled SW is registered automatically by
 * `@serwist/next`'s injected client snippet on first page load in
 * production.
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
import { Serwist } from "serwist";

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

const serwist = new Serwist({
  precacheEntries: self.__SW_MANIFEST,
  // `skipWaiting: false` so the new SW does not steal control
  // until the user actively reloads (or accepts the update toast
  // from `useServiceWorkerUpdate`). Without this gate, a deploy
  // mid-read would silently swap the bundle on the next nav.
  skipWaiting: false,
  clientsClaim: true,
  navigationPreload: true,
  // The default cache set covers Next.js's static assets, font
  // requests, and image responses with sensible strategies. The
  // explicit list of API-route exclusions is enforced by the
  // path-pattern guards below taking precedence over the
  // catch-all default.
  runtimeCaching: defaultCache,
});

// Hard guard: any same-origin request to a backend API surface
// (and the app-route HTML/RSC navigations that share these path
// shapes, e.g. `/series/…`) is never cached and never touched by
// the SW. The list mirrors the documented "DO NOT cache" set
// above; the `fetch` listener below hands every match to the
// browser's native loader.
const API_PATH_PREFIXES = [
  "/admin/",
  "/auth/",
  "/audit",
  "/collections/",
  "/filter-options",
  "/folder-tree",
  "/health-issues",
  "/issues/",
  "/libraries/",
  "/log",
  "/markers/",
  "/me/",
  "/opds/",
  "/rails/",
  "/removed",
  "/scan-preview",
  "/scan-runs",
  "/search",
  "/series/",
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
  const path = url.pathname;
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
