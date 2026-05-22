---
sidebar_position: 1
title: Reader
---

Folio's reader is a keyboard-first comic reader that adapts to manga,
webtoons, and Western comics without you having to think about it. It
offers three view modes, resolves reading direction from a five-layer
chain of signals, supports full gesture input on touch devices, and
operates under a 150 KB gzip bundle budget so that page turns stay
snappy even on slow connections.

:::info[Screenshot]

A hero shot of the reader open on an issue, with the chrome visible,
single-page view mode, dark theme, the whole browser window framed.
Use a recognizable, real-looking comic from the seeded fixture
library so the screenshot does not look staged.

:::

## What it does

The reader lives at its own dedicated route, `/read/<series>/<issue>`.
Whenever you open an issue from anywhere in the app — a library grid,
a series detail page, a saved view, a CBL reading list, a search
result, or an OPDS client — you land here.

The reader is **chromeless by default**: the frame around the page
fades after a moment of inactivity so that the artwork is the focus.
To bring the chrome back, you can move the mouse, tap the middle
third of the page, or press the `c` key. Every option that affects
how you read — view mode, reading direction, fit mode, page strip
visibility — is persisted **per series**, so a choice you make on
volume 1 of a series carries forward to volume 12 without further
intervention.

Six characteristics distinguish the reader from a generic image
viewer:

| Characteristic | What it means in practice |
|---|---|
| Three view modes | Single page, two-up spreads, and continuous webtoon scroll. Switch on the fly with the `d` key. |
| Automatic direction detection | The reader chooses LTR for Western comics and RTL for manga, and a manual override always wins. |
| Direction-aware controls | Arrow keys, tap zones, and double-page pair ordering all mirror automatically in RTL. |
| Touch gestures | Horizontal swipes turn pages; pinches cycle fit modes. |
| Real page strip | A keyboard-toggled thumbnail mini-map scrolls to follow you and lets you jump anywhere in the issue. |
| End-of-issue handoff | When you finish a comic, the reader tells you what to read next, regardless of whether you are inside a CBL, a series, or browsing the rails. |

## Opening an issue and the chrome

The first thing you notice when you open an issue is the chrome.

:::info[Screenshot]

The reader's top and bottom chrome bars over an open issue. The top
bar should show the series name, the issue title, and the page
counter (for example, "Page 12 of 24"). The bottom bar should show
the fit-mode buttons, view-mode buttons, page-strip toggle, and the
settings gear.

:::

The top bar carries the series name, the issue title, and a live
**page counter** that screen readers announce as you turn pages. The
counter is an `aria-live="polite"` region, which means assistive
technology will pick it up without preempting whatever is being read.

The bottom bar groups the controls you reach for most often:

| Control | Purpose |
|---|---|
| Fit mode | Cycle width, height, and original sizing. |
| View mode | Cycle single page, double page, and webtoon. |
| Page strip | Toggle the thumbnail mini-map. |
| Settings | Open the full reader settings panel. |

The chrome auto-hides after several seconds of inactivity so the
artwork has the screen to itself. You can bring it back by moving the
mouse, tapping the middle third of the page, or pressing the `c` key.
Pressing `Esc` from inside the reader exits and returns you to wherever
you opened the issue from.

## View modes

The reader supports three view modes. You can press `d` to cycle
through them, or click the view-mode toggle in the chrome. Your
choice is persisted per series.

### Single page

:::info[Screenshot]

A single-page view of a Western comic, with the page filling the
window. Choose a colorful interior page (not a cover) so that the
mode is clearly identifiable as one full page at a time.

:::

Single page is the default mode. It shows one page at a time,
centered in the window. It is the fastest mode to render, and it
works for every comic regardless of format or aspect ratio.

### Double page (two-up spreads)

:::info[Screenshot]

A double-page view of a Western comic, with two facing pages shown
side-by-side. Ideally pick a spread that is clearly meant to read
across both pages, so that the value of the mode is obvious.

:::

Double page presents two adjacent pages side-by-side, the way
physical comics read. The reader **skips pages flagged `double_page`**
in the ComicInfo metadata so that a full-bleed splash does not get
paired with the following page and shift every spread off-by-one for
the rest of the issue.

In RTL series, the pair ordering flips: page N appears on the right
and page N+1 appears on the left, so that manga spreads read in
their intended order.

### Webtoon (continuous scroll)

:::info[Screenshot]

The reader in webtoon mode, showing a vertical Korean comic with two
or three stacked panels visible. The continuous-scroll layout should
be obvious — no chrome gap between panels, and the panels should
flow into one another.

:::

Webtoon mode stacks every page in the issue vertically and lets the
browser handle native scrolling. It is built for webtoons but is
equally useful for tall single-image comics, or when you simply want
to scroll-read a regular comic without manually paginating.

Touch gestures are disabled in webtoon mode because the vertical
scroll surface already owns touch input. Arrow keys still work for
snap-to-page jumps within the scroll column.

## Fit modes

The reader supports three fit modes, which you can cycle with the `f`
key or by clicking the fit toggle in the chrome. The choice is
persisted per series.

| Mode | Behaviour |
|---|---|
| **Width** *(default)* | The page fills the window's width. If the page is taller than the window, you scroll vertically for the rest of it. |
| **Height** | The page fits the window vertically. If the page is narrower than the window, it is centered horizontally. |
| **Original** | The page renders at its native resolution. This is useful for high-DPI prints or for examining lettering detail. |

:::info[Screenshot]

A triptych comparison: the same page rendered three times, once in
each fit mode, with each variant labelled. Keep the captures
compact so the viewer can compare them at a glance.

:::

On touch devices, **pinching** on a page cycles the fit modes
(pinch out moves toward width, pinch in moves toward original). A
two-finger drag pans within a page when the fit mode is original.

## Reading direction

Folio resolves the reading direction of any series through a
**five-layer chain**, with the most specific signal winning. The
chain is evaluated on every issue load and is automatic by design;
you almost never have to interact with it directly.

| Priority | Source | Where it comes from |
|---|---|---|
| 1 | Per-series localStorage choice | A direction you set on this series in the reader's settings panel. Always wins. |
| 2 | `series.reading_direction` | Set by the admin on the series detail page, or by the bulk-metadata dialog. |
| 3 | User preference | Your `default_reading_direction` under account settings. |
| 4 | Manga heuristic | Series with `Manga=YesAndRightToLeft` in ComicInfo, or auto-detected as manga by the scanner, default to RTL. |
| 5 | Library default | Set when the library was created. |
| 6 | LTR fallback | The final fallback if every other layer is empty. |

You almost never need to touch any of these directly: the chain does
the right thing on the first read of any new series. When it
guesses wrong, you can flip the direction in the reader's settings
panel and the choice will stick for that series indefinitely.

:::info[Screenshot]

The reader's settings panel open as a side sheet, focused on the
Reading direction section. Show the LTR / RTL toggle and the
current-source label (something like "Auto: RTL (from series
metadata)").

:::

Switching reading direction flips three things at once:

| Affordance | LTR behaviour | RTL behaviour |
|---|---|---|
| Arrow keys | `→` advances, `←` retreats. | `←` advances, `→` retreats. |
| Tap zones | Left third = previous, right third = next. | Left third = next, right third = previous. |
| Double-page pair ordering | Page N on the left, page N+1 on the right. | Page N on the right, page N+1 on the left. |

None of these need to be configured separately; the chain hands the
reader a direction and every direction-sensitive affordance follows
from it.

## Navigation

### Keyboard

The reader's keymap is intentionally dense — the reader is the
surface that power users live in, and the keyboard is the fastest
way through it. Pressing `?` inside the reader opens the in-app
reference sheet, which is generated from the live keybind registry
and remains the source of truth for the full inventory.

The most frequently used keys are listed here for quick reference:

| Key | Action |
|---|---|
| `←` / `→` | Previous / next page (flipped in RTL). |
| `Space` | Next page. |
| `Shift+Space` | Previous page. |
| `Home` / `End` | Jump to the first / last page. |
| `gg` / `Shift+G` | Vim-style first / last page. |
| `f` | Cycle the fit mode. |
| `d` | Cycle the view mode. |
| `m` | Toggle the page strip. |
| `c` | Toggle the chrome. |
| `]` / `[` | Jump to the next / previous bookmark in this issue. |
| `Shift+N` | Open the next issue (Up Next). |
| `Shift+P` | Open the previous issue (Previous Up). |
| `?` | Open the keyboard help sheet. |
| `Esc` | Exit the reader. |

Additional bare-key shortcuts exist on the issue page (not in the
reader itself) for `r` (mark read), `u` (mark unread), `b`
(bookmark), `i` (issue info), and `e` (edit metadata). Those are
documented in the Markers and Library feature pages.

### Touch and gestures

:::info[Screenshot]

A diagram (not a photo) of the reader window divided into three
vertical zones, labelled "Previous", "Toggle chrome", and "Next" in
LTR order. Show an RTL variant beside it where the Previous and
Next zones are swapped.

:::

| Gesture | Action |
|---|---|
| Tap the left third | Previous page (flipped in RTL). |
| Tap the right third | Next page (flipped in RTL). |
| Tap the middle third | Toggle the chrome. |
| Horizontal swipe | Advance or retreat one page (30 px threshold). |
| Pinch | Cycle the fit mode. |
| Two-finger drag | Pan within the page when the fit mode is original. |

Gestures are disabled in **webtoon** mode because the vertical scroll
already owns the touch surface there.

### Mouse

| Action | Behaviour |
|---|---|
| Scroll wheel (webtoon mode) | Scrolls the continuous-scroll column. |
| Click in a tap zone | Behaves like a tap. |
| Click on the progress bar in the chrome | Jumps the reader to that page. |

## Page strip

Pressing `m` opens the **page strip**, a vertical mini-map of every
page in the issue.

:::info[Screenshot]

The reader with the page strip overlay open on the right edge of the
window. The strip should show a vertical column of small page
thumbnails; the current page should be highlighted with a ring or
border. If the issue is RTL, ensure the thumbnail order is reversed
so the visual order matches the reading order.

:::

The page strip has four notable properties:

| Property | What it does |
|---|---|
| Lazy-loaded thumbnails | Thumbnails are fetched only as they scroll into view, so opening the strip on a 600-page issue does not pay an upfront cost. |
| Direction-aware ordering | In RTL series, the thumbnails are listed in reverse order so the visual sequence in the strip matches the reading sequence. |
| Reduced-motion aware | The auto-scroll-to-current behaviour uses smooth scrolling by default but jumps instantly when `prefers-reduced-motion: reduce` is set. |
| Click to jump | Clicking any thumbnail navigates the reader to that page. |

To close the strip, press `m` again, click outside the overlay, or
press `Esc`.

## Bookmarks and markers in the reader

Pressing `b` on any page bookmarks that page. Bookmarked pages
display a **Bookmark** chip in the chrome and a small indicator on
their entry in the page strip.

:::info[Screenshot]

The page strip overlay open on an issue that has three bookmarked
pages. Each bookmarked thumbnail should show a small indicator (for
example, a dot or bookmark glyph) in one of its corners, so the
viewer can immediately spot which pages are bookmarked.

:::

You can use `]` and `[` to jump between bookmarks within the issue.

**Marker mode** is a separate and deeper feature for highlighting
specific regions of a page — rectangle selections, text annotations,
and OCR-extracted text from speech bubbles. Because marker mode has
its own keyboard layer and its own server-side pipeline, it gets a
dedicated page: see [Markers](./markers).

## End-of-issue handoff

When you reach the last page of an issue, the reader does not simply
stop. Instead, it presents the **end-of-issue card** with a
suggestion for what to read next.

:::info[Screenshot]

The last page of an issue with the end-of-issue card overlaid in
the centre or bottom of the window. The card should show the
next-up issue's cover, its title, a contextual label (for example,
"Next in [CBL name]") with the position, and clearly labelled
"Continue reading" and "Dismiss" buttons.

:::

The card has three variants depending on where you came from:

| Variant | When it appears | What it suggests |
|---|---|---|
| **CBL** | You opened the issue from a CBL reading list (the URL carries `?cbl=<slug>`). | The next entry in that CBL. If the CBL has updated since you started reading, the card self-heals by re-querying. |
| **Series** | You opened the issue from a series detail page or rail. | The next issue in publication order within the same series. |
| **Caught up** | There is no next entry to suggest in either of the above contexts. | A fallback suggestion drawn from your On Deck rail. |

The card auto-shows on the last page and auto-dismisses if you
navigate back. Pressing `Esc` also dismisses it.

### Up Next and Previous Up

The end-of-issue card uses the same backend resolver as the
corresponding keyboard shortcuts:

| Shortcut | Action |
|---|---|
| `Shift+N` (Up Next) | Jumps to the next issue from anywhere in the reader, not only the last page. |
| `Shift+P` (Previous Up) | Jumps to the previous issue. This is pure sequential back-navigation and does not filter on whether the issue is finished. |

Both shortcuts honour the active CBL context if you opened the
current issue from a reading list.

## Performance

The reader is the **hottest path in the application** because most
user time is spent here. As a result, it has its own performance
budget and rendering pipeline.

| Concern | Approach |
|---|---|
| Bundle size | The `/[locale]/read/[id]` route is gated by a CI script that fails the build if the First Load JS bundle exceeds **150 KB gzip**. `framer-motion`, `@tiptap/*`, and `@dnd-kit/*` are explicitly excluded from this route. |
| Page-turn latency | While you read page N, pages N+1 and N+2 are fetched in the background as `<img>` elements (browser-decoded). The next page turn feels instant once the cache is warm. |
| Decoding | Image bytes are decoded off the main thread via `createImageBitmap` inside a web worker. The main thread stays free for input handling and animation. |
| Animation | Page-turn animations are pure CSS transitions, not driven by a JavaScript animation library. This keeps the bundle thin and the GPU happy on mobile devices. |

## Accessibility

The reader is designed to be usable without a mouse or touch, and to
cooperate with assistive technology rather than fight it. The
following are explicit design constraints:

| Feature | Implementation |
|---|---|
| Page change announcements | The page counter is an `aria-live="polite"` region, so screen readers announce page changes without preempting the user's current task. |
| Keyboard-only navigation | Every gesture has a keyboard equivalent, and every interactive element is reachable via Tab. |
| Focus management | Closing the page strip or settings panel returns focus to the trigger that opened it, so screen-reader users do not lose their place. |
| Reduced motion | Page-strip auto-scroll, end-of-issue card animations, and any transition longer than a flash short-circuit when `prefers-reduced-motion: reduce` is set. |
| Color-independent affordances | The bookmark and current-page indicators use shape and position in addition to color, so the affordance survives a monochrome render. |

A separate accessibility milestone — NVDA, VoiceOver, and
keyboard-only walkthroughs against a seeded fixture harness — is
planned but has not yet shipped. The corresponding audit document is
linked from [Other references](../references).

## Health surfacing

When the scanner has flagged the underlying file for an issue
(recovered archive bytes, skipped entries, decode failures on
specific pages, and so on), the reader surfaces that fact inline so
that you can immediately tell whether a missing page is a problem
with the file or with the application.

:::info[Screenshot]

The reader open on a page with a small health toast pinned in the
bottom-right corner of the window. The toast should say something
like "Recovered from a damaged archive — 3 pages skipped" and
include a "Details" link.

:::

Clicking "Details" opens the issue's health panel, where an admin
can re-validate the file with a deeper recovery scan. In normal
operation you will rarely see this toast; it appears only when the
underlying file actually has an integrity issue.

## Settings panel

The full reader settings panel is reached through the gear icon in
the chrome, or via the **Settings** entry in the keyboard help sheet.

:::info[Screenshot]

The reader settings panel open as a side sheet over the page. The
panel should clearly show its sections: View mode (three radio
options), Reading direction (with a current-source hint), Fit mode
(three radio options), Page strip behaviour (an auto-open toggle),
and an OCR language dropdown for marker mode.

:::

The controls in the panel mirror the chrome buttons but also include
the less-common toggles: page-strip auto-open behaviour, the OCR
language for marker mode, and reading-progress visibility. Every
choice in this panel persists per series.

## Power-user details

For people who prefer to know what is happening under the hood, the
reader exposes a number of details that are not visible in the UI
itself.

| Detail | Behaviour |
|---|---|
| Reading sessions | Each time you open an issue, Folio records a session row with `started_at`, `ended_at`, `pages_turned`, and `time_active_ms`. Idle pauses — for example, a backgrounded tab or more than roughly 30 seconds without input — are excluded from active time. |
| `finished_at` on completion | Reading past the last page of an issue stamps `finished_at` on your progress row. This is what feeds the Reading log's chronological feed. |
| Last-read fact | When you re-open an issue you have been in before, the Issue page hoists a "Last read N hours ago" fact to its top-right so the resume is not a mystery. |
| Per-series localStorage keys | The four per-series persisted choices — `viewMode`, `direction`, `fitMode`, and `pageStripVisible` — are stored in `localStorage` under `reader:<slice>:<series_id>` keys. Clearing site data resets these to defaults but does not affect server-side progress. |

## See also

| Page | Why it is related |
|---|---|
| [Markers](./markers) | Region annotations, OCR-extracted text, and the marker-mode keyboard layer. |
| [Library](./library) | Issue and series detail pages, ratings, bulk metadata editing. |
| [Reading log](./reading-log) | The `/log` page where reading sessions become a feed and a widget-driven activity report. |
| [Saved views](./saved-views) | Filter views such as "Unread manga" or "Started but not finished" that surface what to open next. |
| [Account](./account) | The `default_reading_direction` preference, theme, density, and other defaults that feed into the reader. |
