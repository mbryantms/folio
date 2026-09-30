# PWA hardening and verification

This document tracks the accepted scope of the PWA review. Offline reading,
notifications, and inbound file/share handling are separate features, not
promises made by the offline fallback.

## Implemented scope

- Explicit public-asset caching; API/RSC requests bypass the worker. Known
  legacy private cache buckets are removed on activation. Thumbnail writes
  are serialized with identity-reset deletion to prevent late repopulation.
- Logout/account-change cache clearing and cross-window invalidation.
- A small public offline document; hashed assets cache on demand. No bulk
  precache of administration/reader bundles and no authenticated HTML cache.
- Production-only registration, stale development-worker removal, bounded
  update checks, opt-in reload in the accepting window, Later, and a settings
  update action. Another window's update never reloads the reader.
- Progress retained until 2xx, serialized writes, hidden/pagehide/online flush,
  and removal of pending writes on account reset. Since WP-4.5 every progress
  and reading-session write is also queued in an IndexedDB outbox
  (`web/lib/pwa/outbox.ts`) and replayed on launch, `online`, tab-visible, a
  retry backoff, and (Chromium) a Background Sync wake-up — so a write made
  offline survives terminating the application. See
  `pwa-offline-reading-plan.md` step 5 for the contract.
- Stable manifest identity and Library/Bookmarks shortcuts, install discovery
  with browser-owned prompts or platform instructions, persistent settings UI.
- Generated theme-color values from CSS, client chrome synchronization, dark
  reader chrome, and preservation of the dark-theme WebKit latch workaround.
- Coarse-pointer button sizing, 16px minimum mobile form text, pressed states,
  safe bottom overlay spacing, landscape tab insets, keyboard-aware sheets.
- Sticky safe-area correction only after consistent measurements; keyboard,
  pinch, and windowed geometry excluded. Never unpin during the session.
- Opt-in reading wake lock, native sharing independent of pointer type, share
  cancellation handling with copy fallback, forward-first prefetch, and a
  24-million-pixel retained-image budget (approximately 96 MB RGBA, excluding
  active page images and browser overhead).
- Destination-preserving auth redirects and user-initiated chunk recovery.

## Brand assets (review 5, 6, 9; WP-4.7)

Every declared icon and splash screen now exists. They are generated from
**interim** SVG masters in `web/public/brand/` (an open-book/comic-panel
glyph in the amber `--primary` on the dark `--background`). A production
brand mark is still pending and will replace them without code changes.

- Masters: `web/public/brand/icon-master.svg` plus two 96 px shortcut
  glyphs. `web/public/brand/README.md` has the replacement rules
  (maskable safe-zone radius, tile expectations).
- Outputs: manifest `any` 192/512, opaque full-bleed `maskable` 512,
  Library/Bookmarks shortcut icons, opaque 180 px `apple-touch-icon`,
  18 iOS startup images (9 devices, portrait and landscape), and
  `favicon.ico` (16/32/48) + `icon.svg`. The table is in
  `web/public/icons/README.md`.
- The startup-image device list is `web/lib/pwa/apple-splash-devices.json`.
  The generator and `lib/pwa/apple-splash.ts` (which emits the
  `<link rel="apple-touch-startup-image">` tags) both read it, so files and
  tags cannot drift.
- Next 16 renders `appleWebApp.capable` as `mobile-web-app-capable` only,
  so the root layout adds `apple-mobile-web-app-capable` explicitly for iOS
  releases before 16.4.

### Regenerating

After replacing a master under `web/public/brand/`:

```sh
pnpm --filter web run build-icons
pnpm --filter web exec vitest run tests/pwa/assets.test.ts
```

Then commit everything under `web/public/`. The generator
(`web/scripts/build-icons.mjs`, using `sharp`) reads the background colour
from `lib/pwa/theme-colors.ts`, so regenerate after changing the dark
`--background` token too. Output is about 130 KiB in total. Icons keep full
anti-aliasing and splash screens are palette-quantised.

Install screenshots require synthetic comic/library data and final visual
assets. Never capture real private library content for manifest screenshots.

## Automated checks

- `pnpm --filter web test`: worker bypass/fallback/migration, progress retry and
  ordering, outbox kill-and-relaunch replay and run safety
  (`tests/dom/progress-outbox.test.tsx`), update acceptance, development registration, wake-lock lifecycle,
  safe-area regression, viewport themes, and auth destination validation.
- `pnpm --filter web build`: verifies generated theme colors and compiles the
  actual service worker after Next. Regenerate colors with
  `node web/scripts/theme-colors.mjs` after changing background tokens.
- `PLAYWRIGHT_BASE_URL=http://localhost:8080 pnpm --filter web exec playwright test`:
  production public-origin integration, including desktop and mobile Chromium
  PWA tests. The PWA suite tests cold offline navigation, stale API-cache
  rejection, thumbnail reset, and manifest identity/shortcuts. It also checks
  that every manifest and shortcut icon is served as a PNG at its declared
  size.
- `PLAYWRIGHT_BASE_URL=http://localhost:8080 pnpm --filter web run check-pwa-assets`:
  HTTP status/MIME, `display: standalone`, the `any`/`maskable` split,
  manifest and shortcut PNG dimensions, the Apple touch icon, every startup
  image at its media query's resolution, `apple-mobile-web-app-capable`,
  favicons, worker cache headers, and the offline page. A missing brand
  asset is a hard failure (the former `PWA_REQUIRE_BRAND` opt-in is gone).
- `pnpm --filter web exec vitest run tests/pwa/assets.test.ts` checks the
  same asset inventory statically against `web/public/`, with no server.

## Real-device release checklist (review 20, 22, 38)

Record OS/browser/build and results; emulation is not installed-device proof.

| Surface        | Cases                                                                                                                                         |
| -------------- | --------------------------------------------------------------------------------------------------------------------------------------------- |
| Install/launch | iPhone/iPad Home Screen, Android install, desktop app window; portrait/landscape; cold/warm; dark/light/amber/system                          |
| Geometry       | Notch/home indicator; keyboard open/close; rotation while reading; pinch zoom; iPad floating keyboard, split view and Stage Manager           |
| Navigation     | Android Back with sheet open; iOS edge-back (reader guards it, see below); reader exit after deep-link launch; restore library filters and scroll; auth return destination |
| Overlays       | Install banner, toast, tab bar, sheets and reader strip never cover actionable controls; focused inputs and submit actions stay visible       |
| Accessibility  | VoiceOver/TalkBack, external keyboard, focus restoration, large text/page zoom, reduced motion, Windows forced colors; every theme/accent     |
| Lifecycle      | Hide/lock/kill/resume; progress under failed POST; wake-lock release/reacquisition; permission/battery denial                                 |
| Updates        | Two windows, one reading and one editing; Later; accepting window only reloads; offline/rejected registration; rollback and missing old chunk |
| Identity       | Logout and account switch with a pending thumbnail fetch; another window open; reconnect; no previous-account cache data or queued progress   |

Do not change sticky safe-area pinning based on desktop emulation alone.

iOS edge-back in the reader: an installed iOS web app cancels `touchstart`
in the left 24 px of the page surface so WebKit's swipe-back can't exit the
reader mid-issue (WP-4.2, audit UX-10; details in
[`reader-shortcuts.md`](reader-shortcuts.md#ios-standalone-edge-back-guard)).
Verify on device: an edge swipe turns the page, an edge tap acts as the left
tap zone, and the top-left exit button still works.

## Performance protocol (review 36)

Store measurements in `docs/dev/pwa-performance.md` and attach Playwright JSON
results to the relevant PR. Use the same synthetic issue and device/profile,
record build SHA, OS/browser, viewport/DPR, network/CPU profile, and repeat five
times. Record median and p95; establish budgets from a real baseline.

Measure cold and warm launch to usable library, reader navigation to decoded
first page, input-to-decoded-next-page, request/transfer bytes, worker install
bytes/time, and retained decoded-pixel estimate. Distinguish fixture/desktop
results from low-end physical-device results. Keep the existing reader JS
bundle budget. Do not infer reader latency from the sign-in page's paint time.

## Separate feature plans

See `pwa-offline-reading-plan.md`. Push requires its own subscription/VAPID/job
plan; file/share intake requires an authenticated validation/review workflow.
Window-controls-overlay is intentionally excluded. Custom launch handling is
also deferred: `focus-existing` alone would discard navigation intent without
a `launchQueue` consumer. Continue Reading shortcuts should resolve the user's
current issue only after authentication; no private URLs belong in a manifest.

## Verification completed in this change

- 807 unit tests across 107 files pass.
- Production Next/worker build and TypeScript pass.
- All ten desktop/mobile Chromium PWA integration tests pass, including a real
  waiting-worker activation with two windows and a Later/reopen interaction.
  Final commit preparation repeated the complete PWA suite three times: all
  30 executions passed after correcting the test to target the active toast
  during the dismissed notification's exit animation.
- ESLint has no errors or new warnings; two pre-existing warnings remain in
  IssueActions and LibrarySettingsForm. Query-key, status-color, and current
  reader bundle gates pass.
- Local production HTTP checks confirm worker revalidation, manifest identity,
  and the offline document. Thirteen declared brand assets were missing at the
  time and reported as blockers; WP-4.7 generated them (see "Brand assets"). Rust-origin checks are wired into compose smoke CI;
  no Rust backend or physical installed device was available for the local run.
