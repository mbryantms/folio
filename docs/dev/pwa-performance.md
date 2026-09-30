# PWA performance results

## Baseline from review

The pre-change local build had 254 JS/CSS precache candidates: approximately
9.0 MiB raw and 2.7 MiB gzip. These are local artifact sizes, not measured
network transfers. The new precache is restricted to `offline.html`; Next
assets enter the explicit static runtime cache only when requested.

## Measurement log

No physical-device baseline has been recorded yet. Populate the following
fields using the protocol in `pwa-hardening.md`, and retain raw results with
the PR. Do not replace missing measurements with desktop estimates.

| Build/device/network        | Cold/warm usable library | First readable page | Page turn median/p95 | Transfer bytes | Worker install bytes/time | Retained pixels             |
| --------------------------- | ------------------------ | ------------------- | -------------------- | -------------- | ------------------------- | --------------------------- |
| Pending physical-device run | Not measured             | Not measured        | Not measured         | Not measured   | Not measured              | Budget: 24M prefetch pixels |

## Local verification, 2026-09-18

Production build, local Next server without the Rust backend, headless Chromium.
These single-run smoke observations are **not** an installed-device baseline:

| Profile                    | Worker ready after registration request | Offline document navigation to visible heading |
| -------------------------- | --------------------------------------- | ---------------------------------------------- |
| Desktop Chromium           | 66 ms                                   | 34 ms                                          |
| Pixel 7 Chromium emulation | 54 ms                                   | 44 ms                                          |

The compiled precache contains one document, 1.52 kB. Reader first-load JS at
the time was approximately 190.5 KB gzip across 20 chunks (see "Reader bundle
budget" below for the current number). Backend-dependent reader latency and
physical-device memory remain unmeasured. Raw JSON attachments are emitted in
`web/test-results/results.json` and CI retains that directory, including offline
screenshots and failure traces.

## Reader bundle budget

`web/scripts/check-bundle-size.mjs` gates the reader route's first-load JS in
the `web-check` CI job. **What it measures:** every `static/chunks/*.js` listed
in `.next/server/app/[locale]/read/[seriesSlug]/[issueSlug]/page_client-reference-manifest.js`
(root-layout client components, the route's error / loading / not-found
boundaries, the page), gzipped with Node's default level, summed. The shared framework bootstrap
(`build-manifest.json` → `rootMainFiles`: React DOM, the Next router runtime,
polyfills — about 128 KB gzip, identical on every route) is not counted, and
neither are lazy `next/dynamic` chunks.

| Date / change                          | First-load JS (gzip) | Chunks | Ceiling | Target |
| -------------------------------------- | -------------------- | ------ | ------- | ------ |
| 2026-09-30, `main` before WP-4.4       | 191.20 KB            | 20     | 195 KB  | 150 KB |
| 2026-09-30, WP-4.4 (reader code-split) | 117.45 KB            | 13     | 130 KB  | 120 KB |

The ceiling is measured + 10% (117.45 × 1.1 = 129.2 → 130 KB). Going over the
ceiling fails CI; going over the target only warns. Re-measure after each
reader-feature WP (4.2 zoom/fit, 4.3 spreads/strip, 4.8 page-text panel) and
record the new row here; raise the ceiling only with a justification in this
table.

### What WP-4.4 moved out of first-load

Pattern and rules live in the header of
`web/app/[locale]/read/[seriesSlug]/[issueSlug]/lazy.tsx`: anything not
needed to paint page one is a `next/dynamic(..., { ssr: false })` component
imported from `./lazy`; open-on-demand surfaces mount on first open and
preload on intent.

- **Reader chrome** (top bar with Radix tooltip / popover / dropdown menu,
  Floating UI): starts `data-state="closed"` off-screen and slides in after
  the first frame, so it loads right after hydration without a visible change.
- **Reader settings** (slider, switch, segmented controls): only when the
  gear popover opens; preloaded on hover / focus / pointerdown.
- **Page strip** and **marker overlay** (saved rects + marker / OCR capture):
  mounted unconditionally, load after hydration; marker data comes from a
  client query anyway.
- **End-of-issue card**: mounted (closed) once the reader is within two pages
  of the end, so the chunk is warm and the slide-in still animates.
- **Webtoon end footer** (cover, link, button): webtoon mode only.
- **Global keyboard-shortcuts sheet** (root layout; Radix Dialog + scroll
  lock + focus trap): mounted on first open; `?` still toggles it.
- **Marker mutations**: sharded to `lib/api/mutations/markers.ts` and the
  rail invalidation helper to `lib/api/mutations/rails.ts`. Turbopack does
  not drop unused exports from a reachable module, so importing one hook from
  `@/lib/api/mutations` shipped the whole barrel (~11 KB gzip).

Already lazy before WP-4.4: marker editor, marker-mode pill, first-run
overlay, search modal, service-worker updater.

### What remains (117.45 KB)

Per-chunk attribution from `next experimental-analyze` (module sizes are the
analyzer's per-module gzip estimates):

| Chunk contents                                                              | gzip KB |
| --------------------------------------------------------------------------- | ------- |
| TanStack Query core + `lib/api/queries.ts` (whole module) + query keys      | 19.7    |
| next-intl / ICU message-format parser, QueryProvider, root-layout helpers   | 15.5    |
| `Reader.tsx`, session tracking, keymap, prefetch, `PageImage`               | 14.9    |
| Next client runtime pieces, keybinds, scan events, next-themes              | 12.3    |
| `@use-gesture` (swipe / pinch), reader store, zustand                       | 11.5    |
| sonner (toasts)                                                             | 10.3    |
| tailwind-merge                                                              | 8.3     |
| icons, marker mutations + toggle hook, `lazy.tsx`, URL helpers              | 6.6     |
| Error boundaries (global, `[locale]`, reader) — `StatusScreen` / button dup | 13.4    |
| `not-found`, `loading`                                                      | 4.9     |

Next levers, not taken in WP-4.4: split `lib/api/queries.ts` (8.9 KB, shipped
whole for the same barrel reason as the mutations), trim the ICU parser via
precompiled messages, and dedupe the three error-boundary chunks (each carries
its own copy of `StatusScreen`, `Button`, Radix Slot and CVA).

Icons: `lucide-react` is already resolved per icon (Next's default
`optimizePackageImports`); the analyzer shows only individual
`icons/*.mjs` modules, no barrel pull, so imports stay as named imports.
Vendor chunks shared with the library routes (query, intl, sonner,
tailwind-merge, layout) are the same files, so there is no reader/library
duplication.

### Diagnosing a regression

CI runs `next experimental-analyze --output` before the gate, then
`check-bundle-size --report bundle-report`, which prints the per-chunk table
into the job summary and uploads `reader-bundle-report` (the `reader-bundle.md`
/ `.json` report plus the analyzer's static UI under `analyze/`; serve that
directory with any static server). Locally:

```sh
cd web
pnpm run build
pnpm exec next experimental-analyze --output
node scripts/check-bundle-size.mjs --report bundle-report
```

### First-page preload

`page.tsx` calls React DOM `preload()` for the first page (same `srcSet` /
`sizes` as the `<img>` it will render) with `fetchPriority: "high"`, and for
its strip thumbnail, so both requests start from `<head>` before the reader's
JS arrives.
