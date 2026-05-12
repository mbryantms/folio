# Reader keyboard, gestures, and mode autodetect

Source of truth for the user-facing reader controls. Spec refs in
`comic-reader-spec.md` §7.

## Keyboard

| Key       | Action                                                    |
|-----------|-----------------------------------------------------------|
| `←`       | LTR: previous page · RTL: next page                       |
| `→`       | LTR: next page · RTL: previous page                       |
| `Space`   | Next page (regardless of direction)                       |
| `Esc`     | Exit reader (back to issue detail)                        |
| `m`       | Toggle minimap (page strip at bottom)                     |
| `f`       | Cycle fit mode (`width` → `height` → `original`)          |
| `d`       | Cycle view mode (`single` → `double` → `webtoon`)         |

Tap-zone middle still toggles chrome (header). No keyboard binding for chrome.

## Gestures

Powered by `@use-gesture/react`. Disabled in webtoon mode (vertical scroll
owns the interaction there).

| Gesture                   | Action                                               |
|---------------------------|------------------------------------------------------|
| Swipe left / right        | Next / previous page (direction-aware)               |
| Pinch out / in            | Cycle fit mode                                       |

Threshold for swipe = 30 px horizontal movement. The `prefers-reduced-motion`
media query disables gesture rubber-banding (still discrete page changes).

## Tap zones

Always-on, work without gestures:

```
┌─────────┬─────────┬─────────┐
│  LEFT   │ CHROME  │  RIGHT  │
│  zone   │ toggle  │  zone   │
└─────────┴─────────┴─────────┘
```

Left/right zones are direction-aware: in RTL, the right zone is "previous"
and the left zone is "next". Swipes feel natural in either direction.

## View-mode auto-detect

On first open of a series with no per-series localStorage entry, the reader
picks an initial mode from per-page metadata:

- **webtoon** when median page aspect (height / width) ≥ 2.5 — strong tell
  for vertical strip / webcomic content.
- **double** when ≥ 10 % of pages carry the `DoublePage` flag, OR when
  median aspect indicates landscape spreads (width / height > 1.2).
- **single** otherwise.

User toggles always win and persist per series under
`reader:viewMode:<series_id>` in `localStorage`.

## Direction auto-detect

1. ComicInfo `Manga=YesAndRightToLeft` → **RTL** (always wins).
2. Otherwise, the user's `default_reading_direction` profile preference (set
   via the user menu, stored on `users.default_reading_direction`) →
   `ltr` / `rtl` / null=auto.
3. Fallback → **LTR**.

Per-series localStorage choice (`reader:direction:<series_id>`) overrides
all three when present.

## Mini-map / page strip

Toggled with `m`. Renders a horizontal scrollable strip of small page
thumbnails at the bottom of the reader. Click to jump. Direction-aware
ordering. Active page highlighted with an amber ring; auto-scrolled into
view (smooth unless reduced-motion).

Backed by `GET /issues/{id}/pages/{n}/thumb` — lazy-generated on first
request via the same ZIP LRU as the cover thumbnail. Stored at
`/data/thumbs/<issue_id>/<n>.webp` for `n ≥ 1`; cover (`n = 0`) stays at
`<issue_id>.webp` for backwards compatibility.
