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
  and removal of pending writes on account reset. This buffer is memory-only:
  failed writes do not survive terminating the application.
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

## Brand-dependent items (review 5, 6, 9)

The reviewed raster brand concept is not included in this change; a production
vector master and derived assets are still pending. The manifest and Apple
metadata still declare the planned icons/splash screens. They remain explicit blockers;
do not describe their 404s as a working branded install. Supply the source mark,
generate the declared PNGs, verify maskable safe zones and dimensions, add
landscape/startup coverage for supported devices, and then make
`PWA_REQUIRE_BRAND=1 pnpm --filter web run check-pwa-assets` mandatory.

Install screenshots require synthetic comic/library data and final visual
assets. Never capture real private library content for manifest screenshots.

## Automated checks

- `pnpm --filter web test`: worker bypass/fallback/migration, progress retry and
  ordering, update acceptance, development registration, wake-lock lifecycle,
  safe-area regression, viewport themes, and auth destination validation.
- `pnpm --filter web build`: verifies generated theme colors and compiles the
  actual service worker after Next. Regenerate colors with
  `node web/scripts/theme-colors.mjs` after changing background tokens.
- `PLAYWRIGHT_BASE_URL=http://localhost:8080 pnpm --filter web exec playwright test`:
  production public-origin integration, including desktop and mobile Chromium
  PWA tests. The PWA suite tests cold offline navigation, stale API-cache
  rejection, thumbnail reset, and manifest identity/shortcuts.
- `PLAYWRIGHT_BASE_URL=http://localhost:8080 pnpm --filter web run check-pwa-assets`:
  HTTP status/MIME, manifest PNG dimensions, worker cache headers, Apple assets,
  and offline page. CI reports brand blockers explicitly until assets land.

## Real-device release checklist (review 20, 22, 38)

Record OS/browser/build and results; emulation is not installed-device proof.

| Surface        | Cases                                                                                                                                         |
| -------------- | --------------------------------------------------------------------------------------------------------------------------------------------- |
| Install/launch | iPhone/iPad Home Screen, Android install, desktop app window; portrait/landscape; cold/warm; dark/light/amber/system                          |
| Geometry       | Notch/home indicator; keyboard open/close; rotation while reading; pinch zoom; iPad floating keyboard, split view and Stage Manager           |
| Navigation     | Android Back with sheet open; iOS edge-back; reader exit after deep-link launch; restore library filters and scroll; auth return destination  |
| Overlays       | Install banner, toast, tab bar, sheets and reader strip never cover actionable controls; focused inputs and submit actions stay visible       |
| Accessibility  | VoiceOver/TalkBack, external keyboard, focus restoration, large text/page zoom, reduced motion, Windows forced colors; every theme/accent     |
| Lifecycle      | Hide/lock/kill/resume; progress under failed POST; wake-lock release/reacquisition; permission/battery denial                                 |
| Updates        | Two windows, one reading and one editing; Later; accepting window only reloads; offline/rejected registration; rollback and missing old chunk |
| Identity       | Logout and account switch with a pending thumbnail fetch; another window open; reconnect; no previous-account cache data or queued progress   |

Do not change sticky safe-area pinning based on desktop emulation alone.

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
  and the offline document. Thirteen declared brand assets remain missing and
  are reported as blockers. Rust-origin checks are wired into compose smoke CI;
  no Rust backend or physical installed device was available for the local run.
