"use client";

import dynamic from "next/dynamic";

/**
 * Code-split shim around `<ServiceWorkerUpdater>`. The updater
 * itself imports `@serwist/window` for SW registration + the
 * `waiting` / `controlling` event subscription — a chunk of bytes
 * that has no business loading on a route's critical path,
 * especially the reader's (the §18.1 budget at
 * `web/scripts/check-bundle-size.mjs` caps it at 150 KB gzip).
 *
 * `next/dynamic({ ssr: false })` puts the updater in its own
 * chunk and defers loading until after first paint. Service-worker
 * registration is non-critical for any visible UI — the SW only
 * matters once a deploy happens and a `waiting` event arrives.
 *
 * `loading: () => null` keeps the layout's JSX tree quiet during
 * the short async window; the updater returns `null` itself, so
 * the placeholder and the loaded component render identically.
 */
export const ServiceWorkerLoader = dynamic(
  () =>
    import("./ServiceWorkerUpdater").then((mod) => ({
      default: mod.ServiceWorkerUpdater,
    })),
  { ssr: false, loading: () => null },
);
