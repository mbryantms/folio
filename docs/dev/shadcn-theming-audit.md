# shadcn/ui + theming audit (2026-10-26)

Scope: `web/components/ui/`, `web/components.json`, `web/styles/globals.css`,
the theme runtime (`lib/theme.ts`, `ThemeProvider`, `ThemePicker`,
`ThemeChromeSync`), the shadcn-adjacent dependency set, and how the app
consumes them. Measured against shadcn CLI **4.22.1** and the registry as
served on 2026-10-26.

Verdict up front: the design-token and user-preference layer Folio built
(three themes, four accents, density, status tokens, reader tokens, PWA
safe-area tokens) is **ahead** of stock shadcn and should be kept. The
**component layer and the token format underneath it are two generations
behind** (Tailwind v3-era templates on a Tailwind v4 build), and one of
those gaps is a live bug: every Radix open/close animation in the app is a
no-op. The right move is not "apply a new shadcn theme" but "re-vendor the
primitives from the current `radix-vega` style onto a v4-format token
file, keeping Folio's palette and its patches."

---

## 1. What is installed today

### 1.1 Configuration

| Item | Value | Status |
| --- | --- | --- |
| `components.json` → `style` | `new-york` | Legacy id. Current schema enumerates `{radix,base,aria}-{vega,nova,maia,lyra,mira,luma,sera,rhea}`; `new-york` is still served but no longer the maintained line. |
| `tailwind.baseColor` | `slate` | No longer an offered base color (now `neutral, stone, zinc, mauve, olive, mist, taupe`). Harmless because every token is hand-written, but `migrate base-color` and presets will not understand it. |
| `tailwind.config` | `""` | Correct for Tailwind v4. |
| `iconLibrary` | `lucide` | Current. |
| `menuColor` / `menuAccent` / `rtl` | absent | New fields; defaults apply. |
| Tailwind | 4.3.3 | Current. |
| shadcn CLI | not pinned; `pnpm dlx shadcn@latest` resolves 4.22.1 | `diff` as a standalone command is gone (`add --diff` now). It fails on this tree because `combobox.tsx` is a custom file that shadows a registry name. |

### 1.2 Components in `components/ui/` (40 files)

30 are shadcn-derived, all from the **pre-Tailwind-v4 template**
(first commit 2026-05-12, 45 commits since): `React.forwardRef` +
`displayName`, `React.ElementRef` (deprecated in React 19 types), no
`data-slot` attributes, per-package `@radix-ui/react-*` imports, the old
`focus-visible:ring-2 ring-offset-2` focus model, no `aria-invalid`
styling on `input` / `textarea` / `select`. The current registry template is
plain function components with `data-slot`, `focus-visible:ring-[3px]
ring-ring/50`, `aria-invalid:` states, `[&_svg:not([class*='size-'])]:size-4`
so callers can override icon size, and `xs` / `icon-xs` / `icon-sm` /
`icon-lg` button sizes.

10 are Folio-authored and live in `ui/` by the June audit's F3 decision:
`kbd`, `empty-state`, `native-select`, `filter-pill`, `copy-button`,
`rating-stars`, `CollapsiblePickerSection`, `dropdown-menu-responsive-sub`,
`collision-padding`, `data-table`, plus `combobox`. Four of these now
**collide with upstream registry names** (`kbd`, `empty`, `native-select`,
`combobox`), which is what breaks the CLI diff.

Folio-specific patches carried inside the vendored files (PR 4 moves every
one of them out of `ui/` so the files can be refreshed from upstream
untouched):

- `folio-button` / `folio-control` / `folio-tap` classes for the 44px coarse-pointer tap area (PR #1268).
- `Button size="toolbar"` (36px at 14px text; phone-layout rule 9).
- `DialogContent`: phone bottom-sheet geometry, keyboard inset, `sm:` override rule, viewport height cap, 44px close target.
- `SelectContent` / popovers: `safeAreaCollisionPadding()` so menus clear the status bar and the bottom tab bar.
- `SelectItem`: `hover:` + `focus:` instead of `data-highlighted` (documented Radix tracker gap).
- `Toaster`: top-center under `sm`, bottom-right otherwise; amber → light theme mapping.
- `DialogHeader` / `SheetHeader`: left-aligned at every width.

Usage weight (files importing each): button 152, skeleton 72, badge 63,
input 61, card 40, label 38, alert-dialog 34, dialog 33, dropdown-menu 28,
select 27, tabs 20, sheet 16, switch 15, checkbox 15, popover 14,
filter-pill 14, progress 13, tooltip 12, textarea 12, native-select 11,
empty-state 10, form 9 … accordion 0 (dead), hover-card 1.

Button API actually used: `variant` outline 243 / ghost 132 / secondary 66 /
destructive 13 / link 3; `size` sm 240 / icon 43 / toolbar 20 / lg 5. Only
one file imports `ButtonProps`. 48 call sites override icon buttons with
`h-7 w-7` / `h-8 w-8` / `size-8` because the template lacks `icon-sm`.

### 1.3 Dependencies

| Package | Installed | Latest | Note |
| --- | --- | --- | --- |
| `@radix-ui/react-*` | 17 packages, mixed pin styles | `radix-ui` 1.8.1 (unified) | Internals (`react-primitive` 2.2.0, `react-presence` 1.1.11, …) are already de-duplicated, so there is no bundle bloat today; the unified package is a hygiene and upgrade-cadence win, not a size win. |
| `tw-animate-css` | **absent** | 1.4.0 | Required devDependency of every current style; see F-1. |
| `class-variance-authority` | 0.7.1 | 0.7.1 | Current. |
| `clsx` + `tailwind-merge` | 2.1.1 / 3.7.0 | `cn` 0.5.0 | shadcn moved to a compiled `cn` package in Sep 2026; `lib/utils` re-export is still supported. |
| `cmdk` | 1.1.1 | 1.1.1 | Current. |
| `sonner` | 2.0.8 | 2.0.8 | Current. |
| `next-themes` | 0.4.6 | 0.4.6 | Current. |
| `lucide-react` | 1.61.0 | 1.61.0 | Current. |
| `vaul` | absent | 1.1.2 | Needed only if Drawer is adopted (§4). |
| `@base-ui/react` | absent | 1.9.0 | Base UI became shadcn's default base in Jul 2026. Not recommended here (§3.2). |
| `recharts` | 3.8.1 | registry pins `recharts@3.8.0` | Used raw in 9 files; no `chart.tsx` wrapper. |

No `next/font`; `--font-sans` is undefined, so the whole app renders in the
Tailwind default system stack.

### 1.4 Token system (`styles/globals.css`)

- Format: **HSL channel triplets** (`--primary: 38 92% 55%`) wrapped at use
  site (`--color-primary: hsl(var(--primary))`). This is the Tailwind-v3
  shadcn convention. The current scaffold stores complete colour values
  (`oklch(…)`) and maps `--color-primary: var(--primary)`.
- Radius: `--radius` plus a **3-step** derived scale (`lg = radius`,
  `md = -2px`, `sm = -4px`). The current scaffold derives
  `sm (×0.6) … 4xl (×2.6)` from `--radius`. Any new registry component that
  uses `rounded-xl` / `rounded-2xl` / `rounded-4xl` currently falls through to
  Tailwind's fixed defaults instead of the token, so the compact-density
  radius change (`0.5rem → 0.375rem`) does not reach them.
- Missing standard tokens: `--chart-1..5`, `--sidebar-*` (eight tokens).
  Registry `chart` and `sidebar` items reference them and would render
  unstyled.
- Present, beyond stock: `--success/--warning/--info` (+foreground),
  `--rating`, `--reader-bg`, `--reader-chrome`, five `--density-*` spacing
  vars, safe-area vars, `--topbar-h`, `--bottom-tab-h`, `--keyboard-inset`,
  `--overlay-bottom`, `--top-edge-color`. All of these are good and should
  be preserved verbatim.
- Themes: `dark` (canonical, declared on `:root`), `light`, `amber`;
  accents `blue/emerald/rose` override `--primary/--accent/--ring` (amber is
  the unsuffixed default); `[data-density="compact"]`. `@variant dark` is
  keyed to `[data-theme="dark"]`, not `.dark`, which is a deliberate and
  correct choice for a multi-theme app.
- Guards that encode the HSL format and must change together with it:
  `scripts/theme-colors.mjs` (regex over `--background: H S% L%` to emit
  `THEME_COLORS` for `theme-color` meta), `app/layout.tsx` (`#top-edge`
  uses `hsl(var(--background))` inline), the `@layer base` scrollbar rule
  (`hsl(var(--border))`), `tests/pwa/edge-effect.test.ts`,
  `tests/dom/global-scrollbar-layer.test.ts`.

### 1.5 Theme runtime

Solid and worth keeping as-is: cookie + DB persistence
(`comic_theme/accent/density`, `users.theme/accent_color/density`), SSR
attribute seeding (FOUC-free), `next-themes` with a CSP nonce passed to its
no-flash script, `ThemeChromeSync` keeping `theme-color` / `color-scheme`
meta in step (with the iPadOS status-bar latch handled), `color-scheme` per
theme block, reduced-motion global reset, universal themed scrollbars inside
`@layer base`, forced-colors outlines.

Shadcn's own theming is limited to light/dark via a class; Folio's layer is a
superset. Nothing upstream replaces it.

---

## 2. Findings

Severity: **High** = user-visible defect or blocks every later step;
**Medium** = debt that compounds; **Low** = polish / hygiene.

### F-1 [High] All Radix enter/exit animations are dead CSS

`dialog`, `sheet`, `alert-dialog`, `select`, `dropdown-menu`, `popover`,
`tooltip`, `hover-card`, `command` (9 files) use `animate-in`,
`animate-out`, `fade-in-0`, `zoom-in-95`, `slide-in-from-bottom`,
`slide-out-to-left-1/2`, etc. Those utilities come from
`tailwindcss-animate` (v3) or `tw-animate-css` (v4). Neither is installed and
nothing defines them, so Tailwind v4 silently emits nothing. Verified: the
class names appear in the JS chunks and in **no** built CSS. Every
dialog, menu and tooltip pops in and out with zero transition, and the phone
bottom-sheet "slide up" never slides. The reduced-motion override in
`globals.css` is therefore also moot for these surfaces.

Fix: `pnpm add -D tw-animate-css@1.4.0` and `@import "tw-animate-css";`
after the Tailwind import. The existing `prefers-reduced-motion` rule
(durations forced to 0.01ms rather than `animation: none`) is the right
shape and will keep Radix presence detection working. Expect to re-verify
the bottom-sheet dialogs on iOS, the `SelectionToolbar` grid transition
(unrelated, custom keyframes, fine), and the Playwright reader e2e timing,
because closing now takes ~150–200 ms.

### F-2 [High] Component templates are two template generations old

Consequences the app pays today:

- `input` / `textarea` / `select` have no `aria-invalid` styling. `form.tsx`
  sets `aria-invalid` from react-hook-form errors and the server's
  field-level 422 details bind through `applyServerErrors`, so invalid
  fields are announced but **not visibly outlined** outside `FormMessage`.
- The focus model (`ring-2 ring-offset-2` on `ring-offset-background`) is
  the source of the "clipped rings" class of bugs the owner keeps finding;
  the current model (`focus-visible:border-ring focus-visible:ring-[3px]
  ring-ring/50`, no offset) does not extend outside the box.
- No `icon-sm` / `icon-xs` / `xs` sizes → 48 ad-hoc size overrides.
- `[&_svg]:size-4` is unconditional, so every icon override needs `!`.
- `React.ElementRef` is deprecated in `@types/react` 19.
- No `data-slot` attributes, which the newer composite components
  (`button-group`, `input-group`, `field`, `item`) key their styling on
  (`in-data-[slot=button-group]:rounded-md`). Adding any of them against the
  old primitives produces half-styled results.

### F-3 [Medium] Token format blocks every upstream preset and new component

Because values are HSL channels and the radius scale is 3 steps, nothing
generated by `shadcn/create`, `shadcn apply --preset`, tweakcn, or the
`registry:base` payloads can be pasted in; `chart` / `sidebar` / `field`
would render partly unstyled; and `migrate base-color` does not apply. The
conversion itself is mechanical (every HSL triplet has an exact oklch
equivalent; keep the current colours, do not re-tune) but it touches the
five guard sites listed in §1.4.

### F-4 [Medium] Component inventory gaps vs. what the app hand-rolls

| Upstream item | Folio today | Recommendation |
| --- | --- | --- |
| `spinner` | ad-hoc `animate-spin` Loader icons | Adopt. |
| `kbd` | custom `kbd.tsx` (3 sizes) | Adopt upstream, keep sizes as a `size` variant patch. |
| `empty` | custom `empty-state.tsx` (10 users, `rail` size) | Adopt upstream composition (`Empty`, `EmptyHeader`, `EmptyMedia`, …); keep a thin `EmptyState` wrapper for the 10 call sites. |
| `native-select` | custom (`options` prop, 11 users) | Adopt upstream (`NativeSelect` + `NativeSelectOption`), keep an `options`-prop wrapper so callers do not change. |
| `field` | `form.tsx` (9 users) + hand-rolled label/description/error rows elsewhere | Adopt for new forms; `form.tsx` stays supported. The long admin forms get a cleaner layout vocabulary (`FieldSet`, `FieldGroup`, `FieldLabel`, `FieldDescription`, `FieldError`). |
| `item` | settings rows, session rows, device rows, each hand-built | Adopt for list rows in Settings / Admin. |
| `button-group` / `input-group` | toolbar rows glue buttons with `-ml-px`; 22 raw `<input>`s with absolutely-positioned icons (search boxes) | Adopt; removes a bug class (icon overlapping placeholder at 390px). |
| `toggle-group` / `toggle` | `SegmentedControl` (aria-pressed buttons) | Adopt `ToggleGroup type="single"` under the same `SegmentedControl` API so semantics come from Radix; keep the `folio-tap` and overflow-x rules (rule 9). |
| `drawer` (vaul) | `Dialog` morphs into a bottom sheet via CSS under `sm` | Spike. Vaul adds drag-to-dismiss, snap points, scroll-lock that respects `--keyboard-inset`, and the iOS background-scale affordance. Keep the CSS sheet if the spike shows conflicts with `VisualViewportSync` or the `#top-edge` latch. |
| `chart` | raw recharts in 9 files, tokens via `var(--color-*)` | Adopt `ChartContainer` / `ChartTooltipContent` / `ChartLegendContent` + `--chart-1..5` tokens; the current per-file tooltip styling is duplicated 9 times. |
| `alert` | custom `Banner` in auth forms | Adopt; auth forms keep inline banners per the notifications audit, just on the standard component. |
| `avatar` | none (initials in `UserFooter`) | Adopt, optional. |
| `sidebar` | `MainShell` / `AdminShell` custom `aside` + `useSidebarState` cookie hook + `SidebarTrigger` + `Sheet` on phones | **Adopt** (revised 2026-10-26). Upstream covers everything the custom aside does: `collapsible="icon"` rail, cookie-persisted open state read on the server, Sheet on phones (`setOpenMobile` lets `BottomTabBar`'s More tab open it), `Cmd/Ctrl+B` (register with the shortcut registry). Folio's full-width header above the sidebar is the official "header above" pattern via className overrides (`top-(--topbar-h) h-[calc(100svh-var(--topbar-h))]`), not a file edit. The top-edge element lives in the root layout and is unaffected. What stays app code: the topbar + search, `BottomTabBar`, nav content. |
| `carousel`, `calendar`, `pagination`, `input-otp`, `resizable`, `navigation-menu`, `menubar`, `context-menu`, chat/AI items | not needed | Skip. |
| `accordion` | vendored, 0 users | Delete or keep only if re-vendored for free. |

### F-5 [Medium] Dependency shape

17 individual Radix packages with inconsistent pinning
(`react-dialog` pinned `1.3.1`, most caret). `pnpm dlx shadcn@latest migrate
radix` rewrites `ui/` imports to the unified `radix-ui` package; the two
direct consumers outside `ui/` (`CoverViewer.tsx`,
`RelationshipFormDialog.tsx`) take the path argument. The `cn` package
migration is optional and I recommend skipping it for now: the 0.5.0
package is young, `tailwind-merge` 3.7 is current, and the registry's
`utils` item still resolves to `lib/utils`.

### F-6 [Low] Convention drift (cheap, codemod-able)

- `h-4 w-4` ×375 vs `size-4` ×57 (the June audit's F4; grew since).
- `React.ElementRef` in 7 files → `React.ComponentRef`.
- 139 hard-coded palette classes in 23 files. Most are legitimate
  (reader is an always-dark surface by design; `ThemePicker` accent dots).
  Three are not: `ShortcutsSheet` (9, mounts globally, hard-dark on light
  themes — June F2, still open), `CoverageTab` (9), `MarkersList` (6).
  `check-status-colors.mjs` only guards `(emerald|amber|red|green)-(200|300)`,
  so these slipped through.
- 161 raw `<button>` outside `ui/`. Most are card overlays and
  pressable rows where `Button` is wrong; the ones that should be `Button
  variant="ghost"` are the kebab triggers and chip rows.
- `h-7 w-7` / `h-8 w-8` icon-button overrides (48) go away with `icon-sm`.

### F-7 [Low] No typeface

Every modern shadcn preset ships a font via `registry:font`. Folio uses
the system stack, which is defensible for a PWA (zero font bytes, native
feel on iPad) but means the UI has no typographic identity and `Kbd` /
numeric columns have no mono/tabular setup beyond `tabular-nums`. If a
font is adopted it must be `next/font` self-hosted (same-origin, no change
to the CSP) and declared through `--font-sans` / `--font-mono` in
`@theme inline`.

### F-8 [Low] User-preference surface

What exists (theme / accent / density) covers the high-value axes. Two
cheap additions fall out of the v4 radius scale and the motion rule:

- **Corner radius** (`data-radius="sharp|default|round"` → `--radius`
  0.25 / 0.5 / 0.75rem). Gives users the Lyra-vs-Luma feel without a code
  style switch.
- **Motion** (`data-motion="system|reduced"`), reusing the existing
  reduced-motion block as a selector union with the media query. Useful on
  older iPads once F-1 turns animations on.

Not recommended: a per-user *style* switch (Vega / Luma / …). Styles are
compile-time component code, not runtime tokens; shipping two sets of
`ui/` is not a feature. Also not recommended: a font-size scale (the
`max(16px, 1em)` iOS zoom guard and the 36px row rule are both in px).

---

## 3. Which shadcn style, and which base?

### 3.1 The eight styles, against Folio's constraints

Hard constraints: 36px toolbar controls at 14px text (phone-layout rule 9,
`toolbar-row-height.test`), 44px coarse-pointer tap areas via `::after`
(`touch-control-floor.test`), dense cover grids, dark as the canonical
theme, a reader that stays black, one visible size on every pointer.

| Style | Character (from the registry sources) | Fit |
| --- | --- | --- |
| **Vega** | The classic shadcn look (what `new-york` became). `h-9` default, `rounded-md`, `shadow-xs` outlines, 3px focus ring. | **Best.** Drop-in for every existing size assumption; the re-vendor is a diff, not a redesign. |
| Nova | Vega with tighter padding. | OK, but Folio already has a density toggle doing this at runtime. |
| Maia | Rounded, generous spacing, bordered inputs. | Fights the grid density. |
| Lyra | Square, ignores `--radius`, mono-leaning. | Wrong for a comics app and breaks the radius token. |
| Mira | Densest: `h-7` default buttons, `text-xs`, 2px rings. | **Violates rule 9** (28px rows) and the touch standard. |
| Luma | macOS-Tahoe-like: `rounded-4xl` pill buttons, `rounded-3xl` filled inputs (`bg-input/50`, no border), 30% rings, no shadows. | Attractive for a media app, but pill-shaped 36px controls make every toolbar wider, and borderless filled inputs need re-tuning on all three Folio palettes. Borrow its radius via the user setting instead. |
| Sera | Minimal, typographic, underline inputs, uppercase headings. | Wrong for dense admin tables and forms. |
| Rhea | Compact Luma. | Same objections as Luma, smaller. |

Recommendation: **`radix-vega`** (reconsider Base UI as a separate later step, §3.2). Keep Folio's own palette (there is no
stock base colour that is "amber on blue-black"; the current tokens are the
preset). Use `shadcn/create` only to *preview* ideas; do not `apply` a
preset, because `apply` reinstalls components and would overwrite the
Folio patches listed in §1.2.

### 3.2 Radix vs Base UI vs React Aria

Do the refresh on **Radix** first; treat Base UI as an optional, separate
later step. Reasons, measured:

- Not required: the shadcn team ships every update for both bases and says
  mature Radix apps should keep shipping. Radix is one unified package now.
- The cost is in app code, not `ui/`: `asChild` at **137** sites in 78
  files (Base UI uses a `render` prop), `data-[state=…]` styling at **115**
  sites (Base UI emits `data-open`/`data-closed`; the shadcn base
  stylesheet's `data-open:` variants normalize both, but each site still
  changes), `--radix-*` CSS vars at 14.
- Behaviour differences land on the touch surface: Tabs activate manually
  by default, Select highlight tracking and dialog/menu dismiss + focus
  handling are reimplemented; Select / Dialog bottom sheet / DropdownMenu /
  Tooltip / Tabs need the iPad pass again.
- Radix stays in the bundle regardless: `cmdk` (search modal, palette)
  depends on Radix Dialog, and the registry's Base UI `command` item still
  pulls `cmdk`.
- Counter-point: Base UI is the direction (new-project default; Toast is
  Base-UI-only). With pristine `ui/` files (§4, PR 4) a base switch later
  is a `components.json` change plus `add --overwrite` on the `ui/` side;
  the call-site cost is the same now or later, so sequencing loses nothing
  and keeps two risks apart. Use the official migration skill when/if it
  is done.

---

## 4. Recommended plan

Ordered so each PR is independently shippable and reviewable in a browser.
Each should go out with before/after screenshots at 390 / 820 / 1280 and a
touch pass on iPad, per the owner's polish checklist.

### PR 1 — Turn animations on (F-1)

`tw-animate-css` + import; no component changes. Verify dialogs, sheets,
selects, dropdowns, tooltips, the phone bottom sheet, reduced motion, and the
reader e2e timings. Half a day including verification.

### PR 2 — Dependency hygiene (F-5)

`shadcn migrate radix` (ui + the two external files), remove the 17
`@radix-ui/react-*` packages, pin `shadcn` as a devDependency so the CLI
version is reproducible, update `components.json` (`style: radix-vega`,
`baseColor: neutral` as the nearest valid id, `menuColor`, `rtl: false`).
The registry files import `cn` from the bare specifier `"cn"`; the
published `cn` 0.5.0 package weighs ~10.4 KB gzip and landed in two reader
chunks (bundle gate 143.6 → 152.3 KB), so `web/vendor/cn` is a workspace
package of the same name on clsx + tailwind-merge that every tool resolves
through node_modules — no alias config, files stay pristine.
Move the custom `combobox.tsx` out of `ui/` (e.g. `components/search/`)
so it no longer shadows a registry item; same for `kbd` / `empty-state` /
`native-select` once PR 5 replaces them. Half a day.

### PR 3 — Token file to the v4 scaffold (F-3)

- Convert every HSL triplet to the same colour in oklch (script it; assert
  per-token ΔE ≈ 0, then hand-check the three `theme-color` hex values the
  PWA emits).
- `@theme inline` maps `--color-x: var(--x)` directly.
- Full radius scale (`sm`…`4xl`) derived from `--radius`.
- Add `--chart-1..5` per theme (chart-1 follows the accent; 2–5 from the
  status hues so charts stay readable on amber) and `--sidebar-*` mapped onto
  the existing card/border/primary tokens (cheap insurance even though the
  Sidebar component is not adopted).
- Update the five guard sites (§1.4) and keep `@variant dark` keyed to
  `data-theme`.
- Keep the whole custom token block unchanged.
One to two days, mostly verification.

### PR 4 — Re-vendor the 30 primitives from `radix-vega`, **pristine** (F-2)

Owner decision 2026-10-26: files under `components/ui/` carry **zero
local edits**, so upstream changes can be pulled automatically. Every
Folio patch moves out of the files, which the current templates make
possible because each element carries a `data-slot` attribute:

- **Global CSS on `data-slot` selectors** (`globals.css`, `@layer
  components`): the coarse-pointer tap area (`[data-slot="button"]`,
  `[data-slot="input"]`, `[data-slot="select-trigger"]`,
  `[data-slot="tabs-trigger"]`, … replace `folio-button` / `folio-control`
  / `folio-tap` on upstream components; the classes stay for app-authored
  controls), the `SelectItem` hover rule, the phone bottom-sheet geometry
  and viewport cap on `[data-slot="dialog-content"]`, left-aligned
  `[data-slot="dialog-header"]` / `[data-slot="sheet-header"]`, the 44px
  close target.
- **A thin wrapper layer** (`components/folio/`, imported by app code
  instead of `ui/` — enforce with `no-restricted-imports`): default
  `collisionPadding={safeAreaCollisionPadding()}` on Select / Dropdown /
  Popover / Tooltip content, the `Toaster` position + amber→light mapping,
  `DropdownMenuResponsiveSub`, a `ResponsiveDialog` (upstream's own
  Dialog-on-desktop / Drawer-on-phone pattern, once the vaul spike passes).
- **Call-site changes**: `size="toolbar"` (20 sites) → default size
  (upstream default is already `h-9`); the 48 icon-size overrides →
  `icon-sm`.

Then, family by family, `pnpm dlx shadcn@latest add <item> --overwrite`
with no post-edit: (1) button, badge, input, textarea, label, separator,
skeleton, card; (2) dialog, alert-dialog, sheet; (3) select,
dropdown-menu, popover, tooltip, hover-card, command, collapsible,
accordion; (4) tabs, switch, checkbox, radio-group, slider, progress,
scroll-area, table, sonner, form; (5) sidebar replacing the custom shells'
aside / `useSidebarState` / `SidebarTrigger` (F-4).

Each family is a PR with its own screenshots. The `toolbar-row-height`,
`touch-control-floor`, `toaster-position`, `tabs-edge-fade` and the five
dialog tests are the regression net; `touch-control-floor` must be
re-pointed at the `data-slot` selectors. Five to seven days total.

### PR 4b — Automatic upstream refresh

shadcn has no versioned package for the files; a refresh is `add
--overwrite`. `.github/workflows/shadcn-refresh.yml` (Mondays 06:41 UTC,
or `workflow_dispatch`) runs the pinned CLI over every file under
`components/ui/` and opens or refreshes one PR on the `shadcn/refresh`
branch when the diff is non-empty, pushing with `FOLIO_RELEASE_TOKEN` so
CI runs on it. Typecheck, vitest and the docker-smoke
Playwright job gate it; an upstream API change shows up as a red PR
needing a small manual fix, which is the whole maintenance cost. Pin the
CLI major in the workflow and let Renovate bump it. `radix-ui`, `cmdk`,
`sonner`, `lucide-react` already flow through Renovate.

### PR 5 — Adopt the missing primitives (F-4)

`spinner`, `kbd`, `empty`, `native-select`, `field`, `item`,
`button-group`, `input-group`, `toggle-group`, `alert`, `chart`, then the
`drawer` spike. Wrap the three replaced custom components so their 24
existing call sites do not change in the same PR. Three to five days spread
over several PRs; `chart` and `drawer` are their own.

### PR 6 — Preference additions and a typeface (F-7, F-8)

`data-radius` and `data-motion` on `<html>`, same cookie + `users` column
+ `/me/preferences` path as density (one migration, two columns, two
segmented controls in `ThemePicker`). Pick a font or decide explicitly to
stay on the system stack and record it. One to two days.

### PR 7 — Guards and codemods (F-6)

- Widen `check-status-colors.mjs` to every raw palette class outside the
  reader with an explicit allowlist.
- Codemods: `h-4 w-4` → `size-4`, `React.ElementRef` → `React.ComponentRef`.
- ESLint `no-restricted-imports`: `@radix-ui/*` and `radix-ui` outside
  `components/ui`.
- A `just ui-diff` recipe running `shadcn add <each vendored item> --diff`
  and a `docs/dev/design-system.md` listing every Folio patch per file, so
  the next upgrade starts from a list instead of an archaeology dig.
- Fix `ShortcutsSheet`, `CoverageTab`, `MarkersList` onto tokens.
One to two days.

---

## 5. Risks and how to contain them

- **Visual regression at scale.** PR 4 touches the classes under 152 Button
  call sites. Mitigation: one family per PR, Playwright screenshot diffs on
  the five densest pages, iPad hands-on before merge.
- **The focus-ring model change** (offset ring → inset 3px ring) will
  change how rings sit inside `HorizontalScrollRail` and tab strips. This
  is the fix for the clipped-ring class, but every rail needs a look.
- **Animations arriving** (PR 1) can expose exit-animation races in code
  that assumed instant unmount (the `MainShell` comment about Sheet close
  racing navigation is the known one).
- **Token conversion** must not retune colours. Convert, do not redesign;
  the three themes were AA-tuned per token and the `theme-color` hex values
  are snapshot in `lib/pwa/theme-colors.ts`.
- **`shadcn apply` / presets**: never run against this tree. It reinstalls
  components and would silently drop every Folio patch.

## 6. Out of scope / deliberately not recommended

React Aria; a per-user style switch; the registry's chat/AI/typeset
items; changing the dark-canonical decision or the reader's always-black
surface. Base UI is deferred, not rejected (§3.2).

## Sources

- shadcn changelog: CLI v4 (Mar 2026), Luma / Sera / Rhea, `apply` and
  presets (Apr–May 2026), Base UI as default (Jul 2026), `cn` package
  (Sep 2026), unified `radix-ui` (Feb 2026), `npx shadcn create` and the
  first five styles (Dec 2025) — https://ui.shadcn.com/docs/changelog
- Registry probes on 2026-10-26: `/r/styles/{radix-vega,radix-luma,radix-mira}/{button,input}.json`,
  `/r/styles/radix-vega/style.json` (declares `tw-animate-css` + `shadcn/tailwind.css`),
  `/schema.json` (style and base-colour enums), `shadcn@4.22.1` `tailwind.css`.
- Local: `web/components.json`, `web/styles/globals.css`, `web/components/ui/*`,
  `web/lib/theme.ts`, `docs/dev/frontend-audit-2026-06.md` §F,
  `docs/dev/phone-layout.md` rule 9.
