# Reader keyboard, gestures, and mode autodetect

Source of truth for the user-facing reader controls. Spec refs in
`comic-reader-spec.md` §7.

## Keyboard

The keymap is defined in [`web/lib/reader/keybinds.ts`](../../web/lib/reader/keybinds.ts)
and user-customizable under **Settings → Keybinds**. Press `?` inside
the reader to see the live bindings (the [`<ShortcutsSheet>`](../../web/app/[locale]/read/[seriesSlug]/[issueSlug]/ShortcutsSheet.tsx)
reads from the resolved keymap so user overrides are reflected).

Default bindings — reader scope:

| Default | Action               | Notes                                         |
|---------|----------------------|-----------------------------------------------|
| `→`     | Next page            | Direction-aware (swaps with `←` in RTL)       |
| `←`     | Previous page        | Direction-aware                               |
| `Home`  | First page           | Lands on first spread-group in double-page    |
| `End`   | Last page            | Lands on last spread-group in double-page     |
| `t`     | Toggle controls      | Show/hide chrome (top bar)                    |
| `f`     | Cycle fit mode       | `width` → `height` → `original`               |
| `d`     | Cycle view mode      | `single` → `double` → `webtoon`               |
| `m`     | Toggle page strip    | Show/hide the minimap at the bottom           |
| `Esc`   | Exit reader          | Returns to issue detail                       |
| `b`     | Bookmark this page   | Toggles a page-0 marker on the current page   |
| `n`     | Add note             | Opens the marker editor for a page note       |
| `h`     | Start highlight      | Begins region selection                       |
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
| `comic_reader_next_up_resolved_total` | counter | `source` ∈ {`cbl`, `series`, `none`} | One increment per resolution; lets you see the CBL/series/caught-up mix per user activity. |
| `comic_reader_next_up_latency_seconds` | histogram | none | End-to-end handler latency on every return path (Drop-on-exit timer in [`next_up.rs`](../../crates/server/src/api/next_up.rs)). Default Prometheus buckets cover 5 ms → 10 s — the series-walk worst case (large libraries) lives at the upper end. |
| `comic_reader_prev_up_resolved_total` | counter | `source` ∈ {`cbl`, `series`, `none`} | Sibling of the next-up counter; lets you compare nav direction usage. |
| `comic_reader_prev_up_latency_seconds` | histogram | none | Same shape as the next-up histogram; same `LatencyTimer` instrumentation pattern. |

## See also

- Full audit + recommendations:
  [`docs/dev/keyboard-shortcuts-audit.md`](keyboard-shortcuts-audit.md)
- Settings UI for rebinding: `Settings → Keybinds`
  ([`KeybindEditor.tsx`](../../web/components/settings/KeybindEditor.tsx))
