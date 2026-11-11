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
| 2026-10-15, post-audit T4.4 (account per-series prefs) | 130.13 KB | 14 | 145 KB | 120 KB |
| 2026-10-31, audit C2 (strip pill a11y, OCR alt, RECENTER hoist) | 141.66 KB | 15 | 145 KB | 120 KB |
| 2026-10-31, audit C5 (touch pinch zoom, first-open chrome on touch) | 142.66 KB | 15 | 145 KB | 120 KB |
| 2026-11-04, Folio Flair 1.0 (drag-to-turn, page curl, cover morph into the reader, finishing moment) | 161.47 KB | 17 | 170 KB | 120 KB |
| 2026-11-10, deferred drag-turn motion + peek, health toast, mutations-barrel import | 148.58 KB | 16 | 152 KB | 120 KB |
| 2026-11-11, session refresh retries an unanswered refresh inside the grace window | 152.19 KB | 17 | 153 KB | 120 KB |
| 2026-11-11, first load off the queries barrel, lazy sheet swipe, standalone check sharded | 139.95 KB | 17 | 143 KB | 120 KB |

The ceiling is measured + 10% (117.45 × 1.1 = 129.2 → 130 KB; 130.13 × 1.1 =
143.1 → 145 KB). Going over the ceiling fails CI; going over the target only
warns. The 2026-10-15 row: `main` had crept to 129.37 KB through the
post-audit reader fixes (paired-final-page finish, next-up error states,
collection context + toast, focused-control hotkeys) before T4.4 added the
per-series prefs sync (~0.8 KB: `series-prefs-sync` + the store's mirror /
adopt / push calls). Lazy-loading the API client inside the sync transport
was measured and rejected — the dynamic-import stub costs more than the
import it hides, since `mutations/_core` is already in the reader chunk.
The target is unchanged; the route is 10 KB over it and the next reader
feature should be code-split per `lazy.tsx` rather than lift the ceiling
again. The 2026-10-31 row: `main` stood at 141.47 KB before audit C2 (the
2026-10-15 → 10-31 creep is the reader work in between: offline overrides,
wrap-up, end-of-issue nav). C2 itself adds ~0.2 KB (`usePageAltText` + the
page-text store slot, the `lang` prop on `PageImage`). It first measured
145.91 KB because dropping the four `react-hooks/exhaustive-deps`
suppressions in `Reader.tsx` (N-R12) let the React Compiler compile the
1k-line `Reader` component for the first time — the compiler skips any
function carrying a suppression of a `react-hooks/*` rule — and its
memo-cache code cost 4.3 KB gzip in the reader chunk. `Reader` now opts
out explicitly with `"use no memo"` (same runtime as before, stated rather
than accidental); the page views and hooks in the file stay compiled.
The Flair 1.0 row: `main` measured 143.15 KB at merge time. Flair adds
~18 KB to the reader's first load: momentum drag-to-turn (`use-drag-turn`,
`drag-turn`, the turn peek) and the reworked swipe layer (~3.9 KB), the page
curl hook (the curl engine itself is already a lazy `import()`), the cover
handoff slot in `loading.tsx` (`ReaderCoverMorph` + the effects runtime,
~5.4 KB, which is what lets the cover land without a spinner), the finishing
progress context (series / reading list / collection queries) and the
effects switches. All of these act on the first interaction or the first
paint, so `lazy.tsx` would trade the budget for a visible delay. The ceiling
moves to 170 KB, tighter than the usual +10% (177.6 KB) on purpose; the
target is unchanged, and the next reader work should pay this back by
code-splitting the drag-turn physics and the finishing-progress queries.

The 2026-11-10 row: `main` measured 165.43 KB (17 chunks) before it.
The largest reader-owned item was not an effect but `Reader.tsx` importing
three hooks from the `@/lib/api/mutations` barrel, which shipped the whole
barrel again (a 13.9 KB chunk); `useSetIssueRating` / `useSetIssueNote`
moved to `mutations/issue-user-state.ts` and `useUpdatePreferences` to
`mutations/preferences.ts` (re-exported from the barrel), and the reader
imports the shards. The drag-to-turn motion moved to
`lib/reader/drag-turn-engine.ts`, fetched when the browser is first idle
after the first paint (or on the first drag); until it arrives, or if its
chunk fails, a swipe is the plain threshold turn. The drag peek and the
health toast became `lazy.tsx` surfaces; the peek renders nothing if its
chunk fails rather than reaching the error boundary. The page curl engine
was already lazy (its hook is under 1 KB), and the cover hand-off stays
eager: it must take the cover over in the reader's first commit. The
ceiling drops to measured + ~3 KB = 152 KB. Offline, these chunks are not
in the serwist precache (it holds only `offline.html`); the offline shell
crawl (`lib/pwa/offline-shell.ts`) stores every chunk named inside a
stored chunk, which includes the new lazy ones, and the fallbacks above
cover a cold cache.

The 2026-11-11 row: `main` measured 151.92 KB (17 chunks) in CI before
it, 0.08 KB under the ceiling. The creep since the 2026-11-10 row is
audit remediation that had to run on the first paint: the page states
for missing, unreadable and needs-conversion archives, the page's
transient-failure waits and the page strip's retrying thumbnails. The
change in this row is not reader code at all. It is the session refresh
scheduler, which the root layout loads on every signed-in page and which
this gate therefore counts: it now retries a refresh that got no answer
inside the server's 60-second grace window (`docs/architecture/auth-model.md`),
about 0.3 KB. It cannot be a `lazy.tsx` surface, because it runs exactly
when requests are failing and a chunk fetched at that moment may not
arrive. The ceiling moves by 1 KB to 153 KB, the owner's decision, far
tighter than measured + 10%. That leaves under 1 KB, on purpose: the
next change to anything in first load, reader code or root-layout code,
has to free room first. The candidates named above still stand (the
drag-turn physics and the finishing-progress queries), and the bundle
report has no per-module breakdown without source maps, so that work
starts by producing one. A local build reads about 0.36 KB lower than
CI's for the same tree, so only CI's number decides.

The second 2026-11-11 row pays that back: 152.19 KB → 139.95 KB in CI
(151.83 KB → 139.57 KB in a local build), about 12.2 KB. The per-module view came from a local build with
`productionBrowserSourceMaps: true` (not committed), attributing each
first-load chunk's bytes to its sources through the maps and scaling to
the chunk's gzip size. Do this rather than trust the report's analyzer
column: `--report` pairs analyzer chunks with build chunks by raw size,
and on this tree it put `lib/pwa/downloads.ts` (5.5 KB, which no
first-load module imports) on a chunk that was really the reading
session, keybinds and outbox. The largest item that was not reader code
was `lib/api/queries.ts`, shipped whole (10.1 KB) for the barrel reason
the mutations had: the root layout imported `useMe`, `HttpError` and
`queryKeys` from it, and the reader its own six hooks. Those now live in
shards that the barrel re-exports, so library pages are unchanged:
`lib/api/json-fetch.ts` (`HttpError`, `jsonFetch`), `queries-me.ts`
(`useMe`, `isAuthSurface`) and `queries-reader.ts` (series, CBL, saved
view, markers, spread overrides, next-up / prev-up), and first-load code
takes `queryKeys` from `query-keys.ts` directly. That move alone was
10.43 KB; it is an import change, not a lazy load, so it has no new
failure mode. A test that mocks `@/lib/api/queries` to stub `useMe` for a
component that now reads the shard mocks the shard as well. The other
1.83 KB is root-layout code: `SheetSwipe` fetches the drag-to-dismiss
code after hydration, as `OfflineBootstrap` does (`installSheetSwipe`
wires sheets already open when it runs; if the chunk fails, sheets still
close by button, Escape and overlay, covered by
`tests/dom/sheet-swipe-lazy.test.tsx`), and `isStandaloneDisplay` moved
to `lib/pwa/display-mode.ts`, so `SafeAreaProbe` and `InstallEvents` no
longer pull in the whole pull-to-refresh hook. The new sheet-swipe chunk
is named inside a first-load chunk, so the offline shell crawl stores it.

What stayed, and why: the drag-turn hook and the finishing-progress
queries the earlier rows named are now small (the hook 0.75 KB with its
engine already deferred, `finish-progress.ts` 0.06 KB; the queries cost
was the barrel, now gone). Everything else of size is first paint, first
interaction or failure handling: `Reader.tsx` 11.2, TanStack Query 11.3,
sonner 9.4, `@use-gesture` 9.0 (the first swipe), tailwind-merge 8.5, the
cover hand-off (`ReaderCoverMorph`, `Cover` and the cover helpers, about
9 KB, partly duplicated between the `loading.tsx` and page chunks by the
bundler's chunking), the three error boundaries (about 13.8 KB, each
with its own copy of `Button`, Radix Slot and `StatusScreen`; they must
render when chunks fail), the ICU message parser (about 7 KB), the
reading session, keybinds and outbox (about 6 KB; session timing, the
first key press, offline progress). The next candidates, each needing
more than an import change: dedupe the error-boundary and cover chunks
(a chunking question, not a lazy load), precompiled messages to drop the
ICU parser, and the marker toggle's mutations (about 2.5 KB, used on the
first bookmark tap).

The C5 row: +1.0 KB for the `usePinchZoom` shim, the swipe layer's
`isPinching` gate, `readerTouchAction` and the store's first-open chrome
resolution; the pinch handlers themselves (`lib/reader/pinch-zoom.ts`)
are a dynamic import taken only on touch-capable devices, so they are
not in the first-load chunk.
Re-measure after each
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
