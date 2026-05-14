# Keyboard Shortcuts Audit

**Date:** 2026-05-14
**Scope:** Every keyboard handler in [web/](../../web/) — global hotkeys, the reader keymap, marker overlay, search modal, sidebar toggle, component-level handlers (rating stars, tag inputs, page-jump field), and the user-rebind dialog. Also covers per-user keybind storage and the in-app help surface.
**Verdict:** The reader has a mature, user-rebindable keymap (15 actions, scoped + chord-parsed, with a `?` help sheet). **Outside the reader**, keyboard coverage is sparse and inconsistent:
- Only **two global hotkeys** (`Mod+K` search, `Mod+,` settings) are registered with the global dispatcher; everything else (sidebar `Mod+B`, search-modal arrows, rating-star digits) is hand-rolled per component.
- **No help overlay outside the reader.** A user who lives on `/home` or `/series/...` has no surface that says "press `?` for shortcuts" — the help sheet only mounts inside `/read/...`.
- **No command palette.** `cmdk` is bundled (used as the search modal's combobox primitive) but there is no `Mod+K`-style "do anything" launcher with actions like "go to library", "scan library", "mark issue read".
- **No keyboard-driven navigation** for the library grid, the sidebar list, the issue list, or saved-view rails — Tab traversal works because everything is a link, but there is no arrow-key/J-K vim-style cursor.
- **[reader-shortcuts.md](reader-shortcuts.md) is out of date** — it documents 7 reader keys; the registry now has 15 reader keys plus 2 global keys. Marker-related and global bindings are entirely missing from the doc.
- **No conflicts found** between global and reader scopes (the reader's bubble-phase dispatcher runs after the global capture-phase one, and the marker overlay's capture-phase `stopImmediatePropagation()` correctly shields both).

The plumbing (chord parser, scope mapping, capture/bubble phases, input-hijack gating) is solid. The gap is **coverage and discoverability**, not correctness.

---

## 1. Current implementation

### Architecture

```
                   ┌─────────────────────────────────────┐
                   │ web/lib/reader/keybinds.ts          │
                   │   KEYBIND_DEFAULTS                  │
                   │   KEYBIND_SCOPES (global | reader)  │
                   │   resolveKeybinds(userOverrides)    │
                   │   actionForKey(event, bindings)     │
                   │   parseChord / comboFromEvent       │
                   │   formatKey (⌘ ⌥ ⇧ ⌃ on macOS)      │
                   └──────────────┬──────────────────────┘
                                  │
        ┌─────────────────────────┼───────────────────────┐
        ▼                         ▼                       ▼
┌──────────────────┐   ┌───────────────────────┐  ┌───────────────────────┐
│ GlobalHotkeys    │   │ Reader.tsx            │  │ KeybindEditor.tsx     │
│ (window keydown) │   │ (window keydown,      │  │ (capture-phase recorder│
│ scope: global    │   │  bubble; reader only) │  │  for the settings UI) │
│ actions: 2       │   │ actions: 13 (+2 hard) │  │                       │
└──────────────────┘   └───────────────────────┘  └───────────────────────┘

Per-user overrides live in users.keybinds (JSONB), patched via PATCH /me/preferences.
Surfaced in Settings → Keybinds via KeybindEditor; defaults shipped per-action.
```

- Chord syntax: `Mod+,` / `Ctrl+Shift+f` / `ArrowLeft` / `Space`. `Mod` expands to ⌘ on macOS, Ctrl elsewhere.
- Match function: `actionForKey()` compares a `KeyboardEvent` against the resolved bindings; modifier flags must match exactly (so `Mod+K` does not fire on bare `K`).
- Help sheet: [`ShortcutsSheet`](../../web/app/[locale]/read/[seriesSlug]/[issueSlug]/ShortcutsSheet.tsx) — opened with `?` inside the reader. Lists reader + global sections; reads from the resolved keymap so user overrides are reflected. Not mounted on any non-reader route.

### Per-user storage

- Column: `users.keybinds` (JSONB), action → chord-spec string.
- Patch endpoint: `PATCH /me/preferences` (shipped in M4).
- Editor: [web/components/settings/KeybindEditor.tsx](../../web/components/settings/KeybindEditor.tsx) — modal capture dialog with capture-phase listener so the captured key is not also dispatched globally.

---

## 2. Keybind inventory

### 2.1 Global (window-level, every route)

| Key | Action | Rebindable | Source |
|---|---|---|---|
| `Mod+K` | Open search modal | Yes | [GlobalHotkeys.tsx:60-62](../../web/components/GlobalHotkeys.tsx#L60-L62) |
| `Mod+,` | Open settings | Yes | [GlobalHotkeys.tsx:68-69](../../web/components/GlobalHotkeys.tsx#L68-L69) |
| `Mod+B` | Toggle sidebar collapsed/expanded | **No** | [use-sidebar-state.ts:40-60](../../web/lib/use-sidebar-state.ts#L40-L60) |

`Mod+B` is **not** in the keybind registry — it is a stand-alone window listener inside `useSidebarState`. It is excluded from the help sheet and from `Settings → Keybinds`.

### 2.2 Reader (rebindable, only in `/read/...`)

| Default | Action ID | Label | Source |
|---|---|---|---|
| `→` | `nextPage` | Next page (LTR; flipped in RTL) | [Reader.tsx:378-397](../../web/app/[locale]/read/[seriesSlug]/[issueSlug]/Reader.tsx#L378-L397) |
| `←` | `prevPage` | Previous page (LTR; flipped in RTL) | (same) |
| `Home` | `firstPage` | Jump to first page | (same) |
| `End` | `lastPage` | Jump to last page | (same) |
| `t` | `toggleChrome` | Show/hide top bar | [Reader.tsx:416-417](../../web/app/[locale]/read/[seriesSlug]/[issueSlug]/Reader.tsx#L416-L417) |
| `f` | `cycleFit` | Cycle fit mode | (same) |
| `d` | `cycleViewMode` | Cycle view mode | (same) |
| `m` | `togglePageStrip` | Show/hide page strip | (same) |
| `Escape` | `quitReader` | Exit reader → issue detail | [Reader.tsx:428-429](../../web/app/[locale]/read/[seriesSlug]/[issueSlug]/Reader.tsx#L428-L429) |
| `b` | `bookmarkPage` | Toggle bookmark on current page | (same) |
| `n` | `addNote` | Open marker editor for a page note | (same) |
| `h` | `startHighlight` | Begin region selection | (same) |
| `s` | `favoritePage` | Star/unstar current page | (same) |
| `o` | `toggleMarkersHidden` | Show/hide marker overlays | (same) |

All resolved via [`actionForKey`](../../web/lib/reader/keybinds.ts#L332-L352); merged with user overrides via [`resolveKeybinds`](../../web/lib/reader/keybinds.ts#L143-L155).

### 2.3 Reader (hard-coded, not rebindable)

| Key | Action | Why hard-coded | Source |
|---|---|---|---|
| `Space` | Next page (always) | OS steals it when a button has focus; documenting the override is more useful than allowing rebind | [Reader.tsx:367-370](../../web/app/[locale]/read/[seriesSlug]/[issueSlug]/Reader.tsx#L367-L370) |
| `?` | Toggle `<ShortcutsSheet>` | Help-overlay convention | [Reader.tsx:352-355](../../web/app/[locale]/read/[seriesSlug]/[issueSlug]/Reader.tsx#L352-L355) |

### 2.4 Marker overlay (capture-phase, only while a marker drag is active)

| Key | Action | Source |
|---|---|---|
| `Escape` | Cancel selection (shields reader's `quitReader`) | [MarkerOverlay.tsx:200-214](../../web/app/[locale]/read/[seriesSlug]/[issueSlug]/MarkerOverlay.tsx#L200-L214) |
| `←` `→` `↑` `↓` | Nudge selection 1 % (5 % with `Shift`); bounds-clamped 0–100 | [MarkerOverlay.tsx:220-261](../../web/app/[locale]/read/[seriesSlug]/[issueSlug]/MarkerOverlay.tsx#L220-L261) |

Both use `e.preventDefault() + e.stopImmediatePropagation()` so the reader's bubble-phase listener never sees the event.

### 2.5 Reader chrome — page-jump input

| Key | Action | Source |
|---|---|---|
| `Enter` | Commit jump | [ReaderChrome.tsx:214-225](../../web/app/[locale]/read/[seriesSlug]/[issueSlug]/ReaderChrome.tsx#L214-L225) |
| `Escape` | Cancel edit | (same) |

`stopPropagation()` is critical here so the input's arrows don't drive the reader's page navigation.

### 2.6 Search modal

| Key | Action | Source |
|---|---|---|
| `↓` | Highlight next hit | [SearchModal.tsx:87-93](../../web/components/SearchModal.tsx#L87-L93) |
| `↑` | Highlight previous hit | (same) |
| `Enter` | Open highlighted hit | [SearchModal.tsx:94-107](../../web/components/SearchModal.tsx#L94-L107) |
| `Mod+Enter` | Go to full search page | (same) |
| `Escape` | Close modal (Radix Dialog) | inherited |

### 2.7 Component-level

| Component | Keys | Source |
|---|---|---|
| Rating stars | `0`–`5` set value, `Shift+1..5` half-steps, `↑→` `+0.5`, `↓←` `-0.5` | [ui/rating-stars.tsx:72-87](../../web/components/ui/rating-stars.tsx#L72-L87) |
| Admin tag input | `Enter` / `,` add; `Backspace` on empty removes last | [admin/library/TagInput.tsx:73-78, 406-411](../../web/components/admin/library/TagInput.tsx#L73-L78) |
| Marker tag input | same | [MarkerEditor.tsx:401-412](../../web/app/[locale]/read/[seriesSlug]/[issueSlug]/MarkerEditor.tsx#L401-L412) |
| `QuickReadOverlay`, `SeriesPlayOverlay`, `CoverMenuButton` | `Enter` / `Space` activate (role="button", tabIndex=0) | [QuickReadOverlay.tsx:66-70](../../web/components/QuickReadOverlay.tsx#L66-L70), [CoverMenuButton.tsx:66-70](../../web/components/CoverMenuButton.tsx#L66-L70) |
| `KeybindEditor` capture dialog | Any chord (capture-phase, `preventDefault + stopPropagation`); `Escape` cancels | [KeybindEditor.tsx:216-251](../../web/components/settings/KeybindEditor.tsx#L216-L251) |

### 2.8 Inherited from Radix / cmdk (not custom)

Dialog/Sheet/AlertDialog → `Esc` closes. DropdownMenu → arrows navigate, `Enter`/`Space` select, `Esc` closes. Command (cmdk) → arrows navigate, `Enter` selects, typing filters. Focus traps and `Tab` cycling are also Radix-provided.

### 2.9 Passive listeners (not actions)

| Listener | Purpose | Source |
|---|---|---|
| Reading-session activity tracker | Records keydown timestamps to compute active reading time; `passive: true`, does not consume keys | [reader/session.ts:158](../../web/lib/reader/session.ts) |

---

## 3. Findings

### K-1 — Help surface is reader-only

`<ShortcutsSheet>` is only mounted inside `/read/[...]`. A user pressing `?` on `/home`, `/series/...`, `/bookmarks`, `/views`, `/collections`, `/admin/*`, or `/settings/*` gets nothing. The settings page exists at `Settings → Keybinds`, but it does not show **what is currently bound** the way the reader sheet does — it is an editor, not a cheatsheet.

**Impact:** Users learn `Mod+K` and `Mod+,` only by accident (or by reading `/settings/keybinds`). The two most useful application-wide bindings are invisible.

### K-2 — Sidebar toggle (`Mod+B`) is not in the keybind registry

[use-sidebar-state.ts:40-60](../../web/lib/use-sidebar-state.ts#L40-L60) installs its own window listener. Consequences:
- Not user-rebindable.
- Not displayed in `Settings → Keybinds` or in the reader's `<ShortcutsSheet>`.
- Hard-coded `Mod+B` may collide with browser defaults (Firefox's "bookmarks sidebar") — works in practice because `preventDefault()` claims the chord, but undocumented.

**Fix:** Add `toggleSidebar` to `KeybindAction` with scope `"global"`, register a default of `Mod+b`, and have `useSidebarState` listen via `actionForKey`.

### K-3 — `reader-shortcuts.md` is out of date

[reader-shortcuts.md](reader-shortcuts.md) lists 7 keys (`←` `→` `Space` `Esc` `m` `f` `d`). The current registry ships **15 reader actions** plus 2 global. Missing entirely from the doc: `t`, `Home`, `End`, `b`, `n`, `h`, `s`, `o`, `Mod+K`, `Mod+,`, `?`, and the marker-drag nudges.

**Fix:** Regenerate the table from [keybinds.ts](../../web/lib/reader/keybinds.ts), or replace the hand-maintained list with a one-line "see Settings → Keybinds for the full list, customizable per-user".

### K-4 — No command palette

`cmdk` is already in the bundle (used as the combobox primitive in `SearchModal`), but no "do anything" launcher exists. The search modal can search the catalog, but not invoke commands like:
- "Mark this issue read"
- "Open library settings"
- "Scan library"
- "Toggle dark mode"
- "Go to bookmarks"

A command palette is the single highest-leverage power-user feature missing. It would also paper over K-1 (discovery) and K-5 (no library shortcuts) by letting every action be findable from one keystroke.

### K-5 — No keyboard navigation in lists / grids

The library grid, sidebar nav, saved-view rails, issue list, bookmark list, and collection-detail list are all link-based, so `Tab` works — but there is no:
- Arrow-key cursor (e.g. `↑↓` to walk through issues; `←→` to walk through covers in a rail).
- J/K vim-style next/previous.
- `Enter` to open the focused card with a real focus ring on the cover.
- `Backspace` / `Esc` to step back up the library hierarchy.

Tab traversal hits every focusable element on the page (search field, menu triggers, individual menu items in covers), so reaching the 47th cover requires 47+ tab stops. Power users will not use it.

### K-6 — No `/` quick-search

Convention across many web apps (GitHub, GitLab, YouTube, Discord) is `/` to focus the search field. Folio binds search to `Mod+K` only. `/` is unclaimed; adding it would be free.

### K-7 — Some hard-coded bindings overlap rebindable defaults

- `Escape` is bound to `quitReader` (rebindable) **and** swallowed by Radix Dialog (closes modal first). If the user rebinds `quitReader` to e.g. `q`, `Escape` still closes overlays — good. But if the user rebinds `quitReader` to a key that conflicts with a marker-mode key, there is no validation in `KeybindEditor` for conflicts.
- `s` (favorite page) shares a single keystroke with no modifier, which means in any input field a literal "s" would fire `favoritePage` were it not for the input-hijack gate in `Reader.tsx:348-350`. Hijack-gate works because there is exactly one input in the reader (the page-jump field), and it `stopPropagation`s. **However**, a future reader-overlay input (e.g. quick note) would have to remember to do the same.

**Fix:** `KeybindEditor` should reject a new binding that collides with another action in the same scope. The reader dispatcher should also `target.closest("input, textarea, [contenteditable=true]")` rather than `instanceof HTMLInputElement` so child inputs in overlays inherit protection automatically.

### K-8 — Marker-mode keys are entirely undiscoverable

The marker overlay's `Esc` / arrow / `Shift+arrow` nudges are not in the `<ShortcutsSheet>` or in `Settings → Keybinds`. A user has to read [markers_m7_m8_done.md](../../) or the source to find them.

**Fix:** Add a "While dragging a marker" subsection to `<ShortcutsSheet>` with these four entries (`Esc`, `←→↑↓` nudge 1 %, `Shift+arrows` nudge 5 %).

### K-9 — No conflict validation in `KeybindEditor`

[KeybindEditor.tsx:216-251](../../web/components/settings/KeybindEditor.tsx#L216-L251) records any chord the user presses (except pure modifiers and `Escape`). If two actions in the same scope end up sharing a chord, `actionForKey` returns the first match in registry order — silently. The user is not warned.

**Fix:** On capture, scan `resolveKeybinds(currentOverrides)` for a chord collision. If one exists, surface inline ("This conflicts with **Bookmark this page**. Save anyway?") rather than letting one action silently win.

### K-10 — No "press Tab to focus the page" hint, and no skip-link

There is no "Skip to content" link for keyboard users entering the page. With a long sidebar nav, screen-reader and keyboard-only users must tab past every nav entry to reach the main content. This is an accessibility regression vs. typical SaaS.

### K-11 — Input-hijack gate is `instanceof`-based, not `closest`-based

[Reader.tsx:348-350](../../web/app/[locale]/read/[seriesSlug]/[issueSlug]/Reader.tsx#L348-L350) and [use-sidebar-state.ts:47-52](../../web/lib/use-sidebar-state.ts#L47-L52) check `e.target instanceof HTMLInputElement || HTMLTextAreaElement`. This does **not** match `contentEditable` children inside the focused element, nor inputs inside web components (rare but possible). [GlobalHotkeys.tsx:43-50](../../web/components/GlobalHotkeys.tsx#L43-L50) does check `isContentEditable` — so the three handlers are inconsistent.

**Fix:** Extract a `shouldSkipHotkey(e: KeyboardEvent): boolean` helper used by all three call sites, using `closest("input, textarea, select, [contenteditable=true]")`.

### K-12 — No keyboard control for `<Toaster>` actions

The Undo affordance on toast actions (e.g. "Bookmark removed → Undo") is mouse-only. Sonner does not surface keyboard navigation to toast actions by default, and Folio's `<Toaster>` does not enable `hotkey` config. A keyboard-only user cannot undo a destructive action that uses the Undo pattern.

**Fix:** Enable sonner's `hotkey` prop on `<Toaster>` (default Alt+T) and document it in the shortcuts sheet.

### K-13 — `direction` flip is silent on key remap

In RTL mode, `ArrowRight` becomes `prevPage` and `ArrowLeft` becomes `nextPage` visually. If a user rebinds `nextPage` to `Space`, the flip still applies to the arrow keys (because they remain bound), but the keybind editor does not surface this duality. A user reading both LTR (US comics) and RTL (manga) cannot easily reason about what their bindings will do in either context.

**Fix:** Add a small per-row caption in `KeybindEditor` for arrow-bound actions: "(direction-aware: flips in RTL)".

---

## 4. Proposed keybind matrix

Sorted by impact. Items marked **\*** would require new functionality, not just a new binding.

### 4.1 Global (recommended adds)

| Key | Action | Rationale |
|---|---|---|
| `/` | Focus the search field (or open `<SearchModal>`) | Common web convention; complements `Mod+K` |
| `?` (bare) | Open a **global** `<ShortcutsSheet>` | Discoverability; matches reader convention |
| `Mod+Shift+P` | Open command palette **\*** | VS Code / Cursor / Sublime convention |
| `g` then `h` | Go to Home **\*** (vim-style "go" prefix) | Power-user navigation, optional |
| `g` then `l` | Go to Libraries **\*** | (same) |
| `g` then `b` | Go to Bookmarks **\*** | (same) |
| `g` then `c` | Go to Collections **\*** | (same) |
| `g` then `v` | Go to Views **\*** | (same) |
| `g` then `s` | Go to Settings **\*** | (same) |
| `Esc` | Close any open Sheet/Dialog (already Radix-handled) | Document it |
| `Mod+B` | Toggle sidebar — **register in keybinds.ts** (currently hard-coded) | K-2 |

### 4.2 Library / catalog

| Key | Action | Where |
|---|---|---|
| `↑` `↓` `←` `→` or `j` `k` `h` `l` | Move card cursor **\*** | `/home`, `/series`, library detail, collection detail, saved-view detail |
| `Enter` | Open focused card | (same) |
| `Space` | Play (open reader at resume point) **\*** | (same) — mirrors play-button click |
| `p` | Pin/unpin (on focused saved view) **\*** | `/views`, `/home` rails |
| `i` | Toggle "info" sheet for focused card **\*** | series/issue cards |
| `Esc` | Clear focus / back to grid root **\*** | (same) |

### 4.3 Issue detail (`/series/[slug]/issues/[issueSlug]`)

| Key | Action |
|---|---|
| `Enter` | Read (focused state of the primary CTA) — already works via Tab + Enter |
| `r` | Mark read **\*** |
| `u` | Mark unread **\*** |
| `i` | Open Read in incognito **\*** |
| `b` | Toggle bookmark **\*** (matches reader binding for muscle memory) |
| `e` | Open edit dialog (admin) **\*** |

### 4.4 Reader (additions to the existing 15)

| Key | Action |
|---|---|
| `g` then `g` | Jump to first page (alias for `Home`) — vim convention |
| `Shift+g` | Jump to last page (alias for `End`) — vim convention |
| `[` / `]` | Previous / next bookmark in this issue **\*** |
| `,` / `.` | Previous / next issue in series **\*** |
| `=` / `-` | Zoom in / out (if `cycleFit` modes are extended to a manual zoom) **\*** |
| `Shift+?` | Already maps to `?` because `?` is `Shift+/` — no action |

### 4.5 Search modal additions

| Key | Action |
|---|---|
| `Tab` | Toggle between catalog / saved-views / collections facets **\*** |
| `Esc` | Close (already inherited) |

### 4.6 Saved-view detail page

| Key | Action |
|---|---|
| `e` | Edit view **\*** |
| `r` | Refresh **\*** |
| `p` | Pin to home **\*** |
| `Esc` | Back to `/views` **\*** |

### 4.7 Admin

| Key | Action |
|---|---|
| `Mod+s` | Save current form **\*** (currently every Save is a button click) |

---

## 5. Discoverability assessment

### Should a global help overlay exist?

**Yes.** Promote `<ShortcutsSheet>` to a global component:

1. Move it from `web/app/[locale]/read/[seriesSlug]/[issueSlug]/ShortcutsSheet.tsx` to `web/components/ShortcutsSheet.tsx`.
2. Mount it once in the root layout via a `<GlobalShortcutsSheet>` wrapper that owns its open state.
3. Bind a global `?` (bare key, with input-hijack gate). Keep the reader's `?` working — they can call into the same component.
4. Section the sheet by scope, **and** show the current route's relevant section first (e.g. on `/read/...` show Reader first; on `/home` show Global first).
5. Include the "always-on" entries (Space, ?, marker nudges) as a third section.

### Are contextual shortcut sheets sufficient?

**No.** Today's situation:
- Reader: has `?` → works well.
- Everywhere else: nothing.

A single global sheet that section-folds by scope is simpler than per-page sheets and matches conventions in GitHub (`?`), Linear (`?`), and Notion (`Cmd+/`).

### Is discoverability adequate?

**No.** Specific gaps:
- No surface tells users `Mod+K` / `Mod+,` exist outside of the keybind editor.
- `Settings → Keybinds` is an editor, not a reference. It is also at most three clicks deep, with no entry point unless the user goes hunting.
- The reader's help sheet itself reads "Customize any binding under **Settings → Keybinds**" — useful, but presumes the user is already in the reader.

**Fix:** Add a small "?" button to the bottom-right of the sidebar (or to the user menu) that opens the global shortcuts sheet. Mirrors the GitHub / Linear pattern.

---

## 6. Command-palette opportunity

Given that `cmdk` is already bundled and the search modal already uses it, a command palette is the single largest power-user win and is low-cost:

```
   Mod+Shift+P   →   ┌─ Command palette ────────────────────┐
                     │  >                                    │
                     │  ─────────────────────────────────────│
                     │  Navigation                           │
                     │    Go to Home              g h        │
                     │    Go to Bookmarks         g b        │
                     │  Reader                               │
                     │    Read this issue                    │
                     │    Read from start                    │
                     │    Read in incognito                  │
                     │  Library                              │
                     │    Mark read                          │
                     │    Mark unread                        │
                     │    Add to collection…                 │
                     │    Add to Want to Read                │
                     │  Admin                                │
                     │    Scan library                       │
                     │    Rebuild covers                     │
                     │    Open admin dashboard               │
                     └───────────────────────────────────────┘
```

Each entry is just an `{id, label, scope, action, when}` record. The same registry feeds both the palette and the help sheet, which removes the duplication risk of maintaining a separate cheat-sheet.

Suggested staging:
1. **Phase 1**: Bare palette with navigation entries only (zero risk; just wraps existing routes).
2. **Phase 2**: Add context-aware reader/issue actions (`Mark read`, `Bookmark this page`, …). Each contributes to the palette via a `useCommand()` hook so per-page commands self-register.
3. **Phase 3**: Add admin actions (`Scan library X`, `Rebuild covers`, …) gated by `useMe().role === "admin"`.

This also gives a natural home for the "go to …" `g`+letter sequences (`g h`, `g b`, etc.) — they become `keybinding` fields on the command records.

---

## 7. Missing-coverage matrix

| Area | Has keyboard? | Notes |
|---|---|---|
| Home / catalog grid | ❌ | Tab only |
| Sidebar nav | ❌ | Tab only; toggle works (`Mod+B`) |
| Library detail | ❌ | Tab only |
| Series detail | ❌ | Tab only |
| Issue detail | ❌ | Tab only; no `r`/`u`/`b` shortcuts |
| Reader | ✅ | 15 actions, rebindable |
| Marker overlay | ✅ | Hidden from docs/help |
| Search modal | ✅ | ↑↓ Enter Esc; no facet `Tab` |
| Saved views | ❌ | Tab only |
| Collections | ❌ | Tab only |
| Bookmarks | ❌ | Tab only |
| Settings | ❌ | Tab only; no `Mod+S` save |
| Admin (libraries / scanner / stats / etc.) | ❌ | Tab only |
| Global help overlay | ❌ | Only in reader |
| Command palette | ❌ | `cmdk` bundled but unused for this |

---

## 8. Recommendations (priority-ordered)

| ID | Recommendation | Cost | Priority |
|---|---|---|---|
| R-1 | Promote `<ShortcutsSheet>` to a global component bound to bare `?` (K-1, K-8) | S | **P0** |
| R-2 | Update [reader-shortcuts.md](reader-shortcuts.md) — regenerate from registry or shorten to a pointer (K-3) | XS | **P0** |
| R-3 | Register `Mod+B` (sidebar toggle) in `keybinds.ts` so it appears in the editor + help sheet (K-2) | S | **P0** |
| R-4 | Extract `shouldSkipHotkey(e)` helper used by all three global listeners (K-11) | XS | **P0** |
| R-5 | Add conflict detection to `KeybindEditor` capture flow (K-9) | M | P1 |
| R-6 | Bind `/` to focus search (or open `<SearchModal>`) (K-6) | XS | P1 |
| R-7 | Add marker-mode nudges as a third "While selecting" section in the shortcuts sheet (K-8) | XS | P1 |
| R-8 | Enable sonner's `hotkey` on `<Toaster>` for keyboard-accessible Undo (K-12) | XS | P1 |
| R-9 | Build the command palette (`Mod+Shift+P`) with phase-1 nav entries (§6) | L | P1 |
| R-10 | Add a "?" button to the user menu / sidebar that opens the global shortcuts sheet (discoverability) | XS | P1 |
| R-11 | Add "Skip to main content" link for keyboard users (K-10) | XS | P1 |
| R-12 | Library/grid keyboard cursor (`↑↓←→` or `jklh`, `Enter` to open) (K-5) | L | P2 |
| R-13 | Issue-detail action shortcuts (`r`, `u`, `b`, `i`, `e`) (§4.3) | M | P2 |
| R-14 | Add `[` `]` and `,` `.` to reader (next bookmark / next issue) (§4.4) | M | P2 |
| R-15 | Add "(direction-aware)" caption to arrow rows in `KeybindEditor` (K-13) | XS | P2 |
| R-16 | Add `Mod+S` form-save on settings forms (§4.7) | M | P3 |
| R-17 | Vim-style `g`+letter "go to" sequences for navigation (§4.1) | M | P3 — fold into R-9 |

---

## 9. References

- Registry: [web/lib/reader/keybinds.ts](../../web/lib/reader/keybinds.ts)
- Global dispatcher: [web/components/GlobalHotkeys.tsx](../../web/components/GlobalHotkeys.tsx)
- Reader dispatcher: [web/app/[locale]/read/[seriesSlug]/[issueSlug]/Reader.tsx](../../web/app/[locale]/read/[seriesSlug]/[issueSlug]/Reader.tsx)
- Marker overlay capture-phase listener: [MarkerOverlay.tsx:200-261](../../web/app/[locale]/read/[seriesSlug]/[issueSlug]/MarkerOverlay.tsx#L200-L261)
- Per-user binding storage: `users.keybinds` (JSONB), patched via `PATCH /me/preferences`
- Editor: [web/components/settings/KeybindEditor.tsx](../../web/components/settings/KeybindEditor.tsx)
- Help sheet (reader-scoped today): [ShortcutsSheet.tsx](../../web/app/[locale]/read/[seriesSlug]/[issueSlug]/ShortcutsSheet.tsx)
- Existing (outdated) doc: [reader-shortcuts.md](reader-shortcuts.md)
- Notifications audit (same format): [notifications-audit.md](notifications-audit.md)
