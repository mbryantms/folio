# Reader keyboard, gestures, and mode autodetect

Source of truth for the user-facing reader controls. Spec refs in
`comic-reader-spec.md` §7.

## Keyboard

The keymap is defined in [`web/lib/reader/keybinds.ts`](../../web/lib/reader/keybinds.ts)
and user-customizable under **Settings → Keybinds**. Press `?` from
anywhere in the app to see the live bindings — the global help sheet
([`<GlobalShortcutsSheet>`](../../web/components/GlobalShortcutsSheet.tsx),
mounted at the root layout) opens the route-aware
[`<ShortcutsSheet>`](../../web/components/ShortcutsSheet.tsx) with the
Reader section first when you're at `/read/...` and the Global section
first elsewhere. Both read from the resolved keymap so user overrides
are reflected.

Default bindings — reader scope:

| Default | Action               | Notes                                         |
|---------|----------------------|-----------------------------------------------|
| `→`     | Next page            | Direction-aware (swaps with `←` in RTL)       |
| `←`     | Previous page        | Direction-aware                               |
| `Home`  | First page           | Lands on first spread-group in double-page    |
| `End`   | Last page            | Lands on last spread-group in double-page     |
| `t`     | Toggle controls      | Show/hide chrome (top bar)                    |
| `f`     | Cycle fit mode       | `width` → `height` → `original` → `contain` (fit screen) |
| `d`     | Cycle view mode      | `single` → `double` → `webtoon`               |
| `+`     | Zoom in              | Ladder 1× → 1.5× → 2× → 3×, re-centered; single + double view |
| `-`     | Zoom out             | Walks the same ladder down                    |
| `0`     | Reset zoom           | Back to 1×                                    |
| `m`     | Toggle page strip    | Show/hide the minimap at the bottom           |
| `Esc`   | Exit reader          | Returns to issue detail                       |
| `b`     | Bookmark this page   | Toggles a page-0 marker on the current page   |
| `n`     | Add note             | Opens the marker editor for a page note       |
| `h`     | Start highlight      | Begins region selection                       |
| `x`     | Capture text (OCR)   | Text mode: detected bubbles light up — tap one to OCR it, or drag a box |
| `r`     | Show page text       | Opens the page-text panel: the visible page's OCR text in reading order (see [Accessibility](#accessibility)) |
| `s`     | Favorite this page   | Toggles the star/favorite flag                |
| `o`     | Show / hide markers  | Hides every overlay without deleting data     |
| `]`     | Next bookmark        | Jumps to the next bookmark-kind marker        |
| `[`     | Previous bookmark    | Jumps to the previous bookmark-kind marker    |
| `Shift+N` | Next issue         | Navigates to the resolver's pick (CBL > series); toasts when caught up |
| `Shift+P` | Previous issue     | Sequential back-nav (pure sort-order; ignores read state); toasts at first issue |

Default bindings — global scope (work outside the reader too):

| Default | Action               | Notes                                         |
|---------|----------------------|-----------------------------------------------|
| `Mod+K` | Open search          | `Mod` = `⌘` on macOS, `Ctrl` elsewhere        |
| `Mod+,` | Open settings        |                                               |
| `Mod+B` | Toggle sidebar       | Hidden when typing in an input                |
| `/`     | Open search (alias)  | Common web convention; not user-rebindable    |
| `Alt+T` | Focus latest toast   | Then Tab to action, Enter to fire             |

### Always-on (hard-coded, not rebindable)

| Key       | Action                                | Source                                                                 |
|-----------|---------------------------------------|------------------------------------------------------------------------|
| `Space`   | Next page (regardless of binding)     | OS already claims it for buttons; hard-coded for consistency           |
| `?`       | Toggle the keyboard-shortcuts sheet   | Help-overlay convention                                                |
| `g g`     | First page (alias for `Home`)         | Vim-flavored leader sequence (500 ms window)                           |
| `Shift+G` | Last page (alias for `End`)           | Vim convention                                                         |
| `Ctrl` + scroll | Zoom at the pointer             | Continuous, 1×–3×; also a desktop trackpad pinch. Single + double view (see [Zoom](#zoom)) |
| Double-click | Toggle 2× zoom at the pointer      | Center tap zone; single + double view                                  |

### While drawing a region (mouse held)

These keys are **only active during an in-progress mouse drag** — after
pressing `h` to enter select-rect mode and holding the mouse button to
draw the rect. Once you release the mouse the region is committed and
the marker editor opens; arrow keys no longer reposition it (and won't
nudge a saved marker either).

The marker overlay listens in capture-phase so it sees these keystrokes
before the reader's page-nav handler does:

| Key                 | Action                                    |
|---------------------|-------------------------------------------|
| `Esc`               | Cancel the in-progress drag               |
| `←` `→` `↑` `↓`     | Nudge the in-flight rect by 1 %           |
| `Shift` + arrows    | Nudge by 5 %                              |

The rect is bounds-clamped to `[0, 100]` so a nudge never pushes it off-page.

## Accessibility

WP-4.8 (audit AC-2..AC-5) — what a keyboard or screen-reader user gets:

- **Skip links.** The first two Tab stops in the reader are visually hidden
  until focused: *Show reader controls (T)* reveals the chrome and moves
  focus onto its first button; *Show page text (R)* opens the page-text
  panel. The chrome itself stays hidden and `inert` by default, so these
  are the discoverable way in. The first-run overlay also says, in words,
  that `t` (or a tap in the center) brings the controls back.
- **Page-text panel** (`r`, or the chrome's *Show page text* button). A
  non-modal side sheet listing the visible page's text in reading order
  (rows top to bottom; within a row left-to-right, or right-to-left for
  RTL). It reuses the server OCR surface in
  [`ocr.md`](ocr.md): `GET …/pages/{n}/text-regions` for the detected
  bubbles, then one `POST …/ocr` (`detect: false`) per bubble, two in
  flight at a time, nested line-inside-block boxes collapsed so nothing is
  read twice. Nothing runs until the panel opens — the component is a lazy
  chunk mounted on first open — and results are cached per page and
  region, so flipping back is instant. Because it is non-modal the reader
  keymap keeps working with focus inside it: `←` / `→` turn the page and
  the text follows; `Esc` closes the panel without leaving the reader.
  Manga-recognizer text carries `lang="ja"` so screen readers switch voice.
- **Text-capture proxies.** In text-capture mode (`x`) each detected bubble
  gets a transparent, focusable button in reading order ("Capture text
  region 3 of 7"); Enter runs the same tap-to-OCR capture a pointer tap
  does. The buttons ignore pointer input so drag-select is unaffected.
  Saved region markers already had equivalent proxies (audit E4).
- **Axe.** The reader is covered by the Playwright axe pass (WCAG 2.2 AA
  tags) in `web/tests/e2e/reader-flow.spec.ts`, with the chrome hidden, the
  chrome shown, and the page-text panel open.

## Gestures

Powered by `@use-gesture/react` (drag) plus
[`use-wheel-zoom.ts`](../../web/lib/reader/use-wheel-zoom.ts) (wheel /
trackpad pinch). Swipe is disabled in webtoon mode (vertical scroll
owns the interaction there).

| Gesture                        | Action                                                        |
|--------------------------------|---------------------------------------------------------------|
| Swipe left / right             | Next / previous page (direction-aware)                        |
| Drag while zoomed              | Pan the zoomed page / spread (clamped to its edges)           |
| Double-tap / double-click      | Toggle 2× reader zoom at the tap point (single + double view) |
| `Ctrl` + wheel, trackpad pinch | Reader zoom anchored at the pointer (desktop)                 |
| Touch pinch (phone / tablet)   | Native browser zoom (reads small letterer text)               |

Threshold for swipe = 30 px horizontal movement. The `prefers-reduced-motion`
media query disables gesture rubber-banding (still discrete page changes).

**Pinch behavior change (v0.3.21+):** pinch used to cycle fit modes;
touch pinch now defers to native browser pinch-to-zoom so users can
zoom in on small text. The fit-mode cycle stays bound to the `f` key
and the chrome toggle button. While the page is natively zoomed
(`visualViewport.scale > 1`) the swipe-to-turn handler is
suppressed so single-finger pans go to the browser viewport
instead of accidentally turning pages.

### Zoom

Reader zoom is a CSS transform on the page (single view) or the whole
spread (double view), so both pages of a pair and their marker overlays
scale and pan together. Webtoon has no reader zoom (it is width-fit by
construction; ctrl+wheel falls through to the browser there).

- **Desktop pinch / ctrl+wheel (WP-4.2, audit UX-3).** Chromium,
  Firefox and Edge report a trackpad pinch as a `wheel` event with
  `ctrlKey`; macOS Safari reports it as `gesture*` events. Both are
  claimed (`preventDefault`) so they no longer trigger *browser* page
  zoom — which used to scale the fixed chrome and page strip along
  with the art — and instead zoom the page continuously (1×–3×,
  exponential in the wheel delta, one mouse notch ≈ ×1.28) keeping
  the content under the pointer stationary. `gesture*` events are
  only handled on fine-pointer devices; on iOS they also fire for a
  two-finger touch pinch, which stays native.
- **Keep zoom between pages.** Off by default: zoom resets to 1× on
  every page turn. With **Reader settings → Display → Keep zoom
  between pages** on (persisted globally in localStorage as
  `reader.v1:zoomPersist:_default`), a page turn keeps the zoom level
  and lands on the new page's reading-order start corner — top-left in
  LTR, top-right in RTL. Changing fit mode, view mode, or entering a
  marker-selection mode always resets to 1×.
- Math lives in [`zoom.ts`](../../web/lib/reader/zoom.ts)
  (`zoomAboutPoint`, `clampZoomPan`, `zoomAfterPageTurn`), with unit
  tests in `web/tests/reader/zoom.test.ts`.

### iOS standalone edge-back guard

In an installed (Home Screen) iOS / iPadOS web app, WebKit turns a
rightward swipe that starts at the left screen edge into history-back,
which used to exit the reader mid-issue (audit UX-10). In that mode
only (`navigator.standalone === true`), a touch that starts in the
left 24 px of the page surface (`EDGE_BACK_INSET_PX`) cancels its
`touchstart`, which suppresses the OS gesture; the swipe still reaches
the reader, so an edge swipe turns the page like any other swipe. A
plain tap in that strip is re-dispatched as the left tap zone (the
cancelled `touchstart` swallows the synthesized click). The guard is
scoped to the page surface (`data-edge-guard` on the tap zones and the
webtoon tap layer), so chrome buttons at the edge keep working; in
webtoon a tap in the strip does nothing. Browser Safari (not
installed) and Android / desktop PWAs are unaffected. Implementation:
[`use-swipe.ts`](../../web/lib/reader/use-swipe.ts).

## Fit modes

| Mode       | Behavior                                                                 |
|------------|--------------------------------------------------------------------------|
| `width`    | Page fills the viewport width (scales up if narrower); may scroll down   |
| `height`   | Page fills the safe viewport height; may overflow (and pan) sideways     |
| `original` | Intrinsic pixel size                                                     |
| `contain`  | **Fit screen** — the whole page visible, scaled up or down (audit UX-4)  |

`contain` sizes the image to `min(available width, safe viewport height ×
aspect)` using a `--page-ar` custom property PageImage sets from the
server-known page dimensions (or the decoded size), rather than
`object-fit` — the img box stays the rendered art, so marker overlays
still align. In double view each page of a pair is capped at half the
viewport width; webtoon treats `contain` as `width`. It is selectable
per series (reader settings, `f`) and as the account default fit
(`Settings → Reading`, stored as `default_fit_mode = "contain"`).

## Progress bar

The thin progress bar under the top chrome mirrors in RTL (audit
UX-5): it fills from the right edge, matching page-turn direction and
the page strip. The reported value (`aria-valuenow`) is the same in
both directions.

## Tap zones

Always-on, work without gestures:

```text
┌─────────┬─────────┬─────────┐
│  LEFT   │ CHROME  │  RIGHT  │
│  zone   │ toggle  │  zone   │
└─────────┴─────────┴─────────┘
```

Left/right zones are direction-aware: in RTL, the right zone is "previous"
and the left zone is "next". Swipes feel natural in either direction.

## View-mode auto-detect

On first open of a series with no per-series localStorage entry, the reader
resolves the initial view mode (`detectInitialViewMode` in
[`detect.ts`](../../web/lib/reader/detect.ts)):

1. `series.reading_direction = "ttb"` (the series editor's
   **Vertical (webtoon)** option) → **webtoon** (WP-4.2, audit UX-5).
   Series-level intent, so it beats the account default below.
2. The user's `default_view_mode` preference.
3. Page metadata:
   - **webtoon** when median page aspect (height / width) ≥ 2.5 — strong
     tell for vertical strip / webcomic content.
   - **double** when ≥ 10 % of pages carry the `DoublePage` flag, OR when
     median aspect indicates landscape spreads (width / height > 1.2).
   - **single** otherwise.

User toggles always win and persist per series under
`reader:viewMode:<series_id>` in `localStorage`.

## Double-page pairing and manual spread controls

In double-page view the reader walks the issue into *spread groups*
(`web/lib/reader/spreads.ts::computeSpreadGroups`): the cover solo
(when "First page is cover" is on), a page that reads as a spread solo,
everything else in left/right pairs. A page reads as a spread when
ComicInfo flags it `DoublePage` or its aspect ratio is ≥ 1.2.

Offset scans and unflagged spreads defeat both signals, so each user can
correct the pairing per issue (WP-4.3). Overrides **win over** the
`DoublePage` flag and the aspect heuristic:

| Control | Where | Effect |
|---|---|---|
| Per-page mode pill (`Auto` → `Spread` → `Single`) | Page strip, double view only; always shown on overridden and on-screen pages, on hover elsewhere | `Spread`: the page always renders alone. `Single`: the page pairs like an ordinary page even when flagged or landscape. |
| "Shift pairing by one" | Settings popover → Spreads | The first page that would start a pair renders solo instead, so every later pair moves by one page (fixes an offset scan). A page that is solo anyway (next to a spread) doesn't consume the shift. |
| "Reset spread overrides for this issue" | Settings popover → Spreads | Back to automatic pairing. |

Overrides are stored server-side per `(user, issue)` in
`issue_page_overrides` (`GET`/`PUT`/`DELETE
/api/me/issues/{issue_id}/page-overrides`, handler
`crates/server/src/api/page_overrides.rs`), so they follow the user to
every device. `PUT` replaces the whole set; an all-default body deletes
the row. The endpoints enforce the issue's library ACL and age-rating
cap (an invisible issue is a 404). The read page prefetches the row
during SSR so the first paint already pairs with it; the page strip,
reader and settings popover share one TanStack query
(`queryKeys.issuePageOverrides`) and the mutations update it
optimistically. There is no keyboard shortcut yet; the strip pill for
the on-screen page(s) is a tab stop.

## Direction auto-detect

Five-layer resolution chain (highest-priority first), shipped in
`~/.claude/plans/manga-and-bulk-metadata-1.0.md` M1+M2:

1. ComicInfo `Manga=YesAndRightToLeft` on the issue → **RTL** (always
   wins — author intent).
2. `series.reading_direction` override → admin-set per-series, or
   auto-set by the M3 scanner heuristic when ≥80% of the series's
   issues declare manga.
3. The user's `default_reading_direction` profile preference (set
   via the user menu, stored on `users.default_reading_direction`)
   → `ltr` / `rtl` / null=auto.
4. The parent library's `default_reading_direction` (newly consulted
   by M1).
5. Fallback → **LTR**.

Unrecognized values at any layer (`"auto"`, etc.) are treated as "no
opinion" and the chain falls through to the next signal. The series
layer's `"ttb"` is a layout choice — it selects webtoon view (see
[View-mode auto-detect](#view-mode-auto-detect)) — so it is also "no
opinion" for page-turn direction.

Per-series localStorage choice (`reader:direction:<series_id>`)
overrides all five when present — client-side only, not synced
across devices. Server-side per-series sync is deferred (R4).

## Mini-map / page strip

Toggled with `m`. Renders a horizontal scrollable strip of small page
thumbnails at the bottom of the reader. Click to jump. Direction-aware
ordering. Active page highlighted with an amber ring; auto-scrolled into
view (smooth unless reduced-motion).

Backed by `GET /issues/{id}/pages/{n}/thumb` — lazy-generated on first
request via the same ZIP LRU as the cover thumbnail. Stored at
`/data/thumbs/<issue_id>/<n>.webp` for `n ≥ 1`; cover (`n = 0`) stays at
`<issue_id>.webp` for backwards compatibility.

## Next-issue resolver

`Shift+N`, the end-of-issue card (auto-shown on the last page), and
`Shift+P` (back-navigation) all ask the same family of server
resolvers. The endpoints are `GET /issues/{issue_id}/next-up?cbl=<saved_view_id>`
and `GET /issues/{issue_id}/prev-up?cbl=<saved_view_id>`. Both share
the response shape (`NextUpView`) and resolution order:

1. **CBL** — if `?cbl=<saved_view_id>` resolves to a saved view the
   user can see with `kind='cbl'` AND the current issue is in that
   list, return the next-unfinished entry after the current position.
2. **Series** — otherwise (or after a CBL fallthrough), walk the
   current issue's series in sort order and return the first
   ACL-visible not-finished issue strictly after the current one.
3. **None** — both branches dry: the response carries `source: "none"`
   and the end-of-issue card renders the caught-up empty state.

The CBL context is carried by the `?cbl=` query param on the reader
URL — produced by every CBL → reader/issue link (`<CblIssueCard>`,
`<CblWindowCard>`, the CBL detail page). When the resolver picks a CBL
next, the next reader URL forwards the param; a series fallthrough
strips it so the reader resets to series-only context.

When the param is *stale* (CBL exists but the current issue isn't in
it — e.g., the entry was deleted), the server returns
`cbl_param_was_stale: true` and the web layer strips `?cbl=` from the
current URL via `router.replace`, so a page refresh / shared link no
longer carries the dead reference.

### prev-up semantic differences

`prev-up` mirrors the URL contract and response shape but has two
behavioral differences vs. `next-up`:

1. **No `finished` filter.** Prev is pure sequence navigation — a
   user pressing `Shift+P` is asking to back up one step, not to
   find an unread issue. If the user is on issue 5 and issues 3-4
   are already finished, `prev-up` returns issue 4.
2. **`fallback_suggestion` is never populated.** "You're already at
   the start, here's an unrelated suggestion" doesn't make sense;
   the field stays null for prev results.

Resolver, helpers, and tests live in
[`crates/server/src/api/next_up.rs`](../../crates/server/src/api/next_up.rs)
(`next_up` + `prev_up` handlers share the file); the web side is the
`useNextUp` / `usePrevUp` hooks in
[`web/lib/api/queries.ts`](../../web/lib/api/queries.ts).

### Resolver telemetry

Two Prometheus metrics exposed at `/metrics`:

| Metric | Type | Labels | What it measures |
|---|---|---|---|
| `folio_reader_next_up_resolved_total` | counter | `source` ∈ {`cbl`, `series`, `none`} | One increment per resolution; lets you see the CBL/series/caught-up mix per user activity. |
| `folio_reader_next_up_latency_seconds` | histogram | none | End-to-end handler latency on every return path (Drop-on-exit timer in [`next_up.rs`](../../crates/server/src/api/next_up.rs)). Default Prometheus buckets cover 5 ms → 10 s — the series-walk worst case (large libraries) lives at the upper end. |
| `folio_reader_prev_up_resolved_total` | counter | `source` ∈ {`cbl`, `series`, `none`} | Sibling of the next-up counter; lets you compare nav direction usage. |
| `folio_reader_prev_up_latency_seconds` | histogram | none | Same shape as the next-up histogram; same `LatencyTimer` instrumentation pattern. |

## See also

- Full audit + recommendations:
  [`docs/dev/keyboard-shortcuts-audit.md`](keyboard-shortcuts-audit.md)
- Settings UI for rebinding: `Settings → Keybinds`
  ([`KeybindEditor.tsx`](../../web/components/settings/KeybindEditor.tsx))
