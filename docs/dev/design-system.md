# Design system — shadcn/ui, vendored

How the web UI layer is put together after the 2026-10 modernization
([`shadcn-theming-audit.md`](shadcn-theming-audit.md) has the audit and
the plan; this page is the operating manual).

## The three layers

| Layer                  | Path                                     | Rule                                                                                                                                                                                                                                                                                                                                                 |
| ---------------------- | ---------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Vendored**           | `web/components/ui/*`, `web/lib/hooks/*` | Byte-for-byte registry files (`shadcn add --overwrite`, style `radix-vega`). Never hand-edited: prettier-ignored, ESLint-ignored, `linguist-vendored`. Refreshed weekly by `.github/workflows/shadcn-refresh.yml`.                                                                                                                                   |
| **Folio wrappers**     | `web/components/folio/*`                 | Thin modules that re-export a vendored file and override one or two pieces with Folio behaviour. App code imports these for the modules listed below; ESLint `no-restricted-imports` blocks the `ui/` path for them. Also home to the Folio-authored primitives (`filter-pill`, `copy-button`, `rating-stars`, `data-table`, `combobox`, `form`, …). |
| **Tokens + CSS hooks** | `web/styles/globals.css`                 | oklch tokens for the three themes, four accents and density; the `@theme inline` map; Folio geometry keyed on `data-slot` selectors.                                                                                                                                                                                                                 |

Everything else (`components/library`, `components/admin`, …) is app code.

## What each wrapper adds

| Wrapper                                                                     | Adds                                                                                                                                                                                                                                                                                     |
| --------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `folio/select`                                                              | Safe-area `collisionPadding` on popper content; full-width trigger by default.                                                                                                                                                                                                           |
| `folio/dropdown-menu`, `folio/popover`, `folio/hover-card`, `folio/tooltip` | Safe-area `collisionPadding`. Popover also keeps the tall-popover scroll cap and `PopoverPortalContainer`, which flips descendant popovers to Radix `modal` inside a dialog. Tooltip carries its own provider so the package stays out of routes that show none (reader bundle gate).    |
| `folio/tabs`                                                                | `TabsList` defaults to the `line` variant (full-width strip on a hairline, accent underline from `globals.css`); mode pickers pass `variant="default"` for the segmented pill with a solid primary segment. Scrolls on the x axis only, hides its scrollbar, fades the overflowing edge. |
| `folio/sonner`                                                              | Toaster placement (top-centre on phones), safe-area offsets, amber → light theme mapping.                                                                                                                                                                                                |
| `folio/scroll-area`                                                         | `viewportClassName`, `viewportProps`, `horizontal` composed over the upstream `ScrollBar`.                                                                                                                                                                                               |
| `folio/sidebar`                                                             | Provider + the rebindable `toggleSidebar` keybind; rail positioned under the full-width topbar and `sticky` (not upstream's `fixed`, which iOS holds still during the rubber-band bounce); phone sheet closes on navigation; `useSidebarCollapsed()`.                                    |
| `folio/kbd`, `folio/empty-state`, `folio/native-select`                     | Folio's call-site APIs (keycap sizes, one-call empty state, `options` list) over the upstream items.                                                                                                                                                                                     |

## What the CSS hooks do (`globals.css`)

- **Touch targets** (phone-layout rule 9): under `(pointer: coarse)` every control gets a transparent 44px `::after` tap area, matched by `data-slot` for vendored controls and `.folio-control` / `.folio-tap` for Folio-authored ones.
- **Overlays**: the phone bottom-sheet dialog, the short-window height cap, sheet scrolling, the safe-area-aware sheet close, left-aligned alert-dialog headers.
- **Select options**: plain `:hover` highlight for the mouse.
- **Tabs**: the line variant's indicator in `--primary` (upstream draws it in the foreground colour), the default variant's selected segment as a solid primary pill. Tab triggers carry their tap area on `::before` because upstream's indicator owns `::after`.
- **Toasts**: Sonner's injected stylesheet is unlayered, so the action / cancel button palette (primary / secondary) lives in unlayered rules that outrank Sonner's by specificity; the token mapping itself is restated in `folio/sonner`.
- **Sidebar**: icon rail content scrolls vertically; the phone sheet clears the safe areas.

A doubled attribute selector (`[data-slot="x"][data-slot="x"]`) outranks the vendored files' single-class utilities inside the same cascade layer, so these rules win without `!important` and callers' `sm:` overrides keep working.

## Recipes

**Add a component.** `cd web && pnpm exec shadcn add <item>`. If Folio needs behaviour on top, write `components/folio/<item>.tsx` (re-export + override), add the item to the ESLint restricted list, and import the wrapper from app code. Never edit the `ui/` file.

**Refresh from upstream now.** `just ui-refresh` runs the same pull the weekly workflow does. Review the diff, run `just check`, commit.

**Preview an upstream change.** `pnpm exec shadcn add <item> --diff`.

**Style a vendored element.** Target its `data-slot` from `globals.css`; prefer a wrapper when the change is behavioural (props, portals, providers).

**Status colours.** Use the tokens (`text-success`, `bg-warning`, `text-info`, `bg-destructive`) or `statusTone()`; `pnpm run check-status-colors` fails CI on any raw palette class outside the allowlisted files (reader surfaces, the accent swatches, the marker palette).

## Don'ts

- Don't run `shadcn apply` or `init --preset`: both reinstall components and would discard the wrappers' assumptions.
- Don't add a `TooltipProvider` to the root layout (see the tooltip wrapper).
- Don't put Folio classes or props into `components/ui/*`; the next refresh removes them.
