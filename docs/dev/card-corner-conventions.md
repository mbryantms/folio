# Cover-card corner conventions

The cover cards across Folio (`IssueCard`, `SeriesCard`, `CblIssueCard`,
`CblWindowCard`, `ProgressIssueCard`, `OnDeckCard`) share a small set of
absolute-positioned children. To keep the reading experience consistent
across rails, grids, detail pages, and search, each corner has a fixed
purpose.

## Slot contracts

| Slot | Purpose | Default occupant |
|---|---|---|
| **Top-left** | Affordance — actions / selection | `CoverMenuButton` (kebab, hover-reveal). `SelectionCheckbox` overlays it with higher z-index when `selectMode` is active. |
| **Top-right** | Highest-priority **state** signal | See priority cascade below. |
| **Bottom-left** | Card-type-specific **context** badge | Different per card: see "Bottom-left semantics" below. |
| **Bottom-right** | Affordance — **play / open** | `QuickReadOverlay` / `SeriesPlayOverlay` (hover-reveal). Always reserved for this — never put state indicators here. |
| **Bottom-full** | Continuous **progress** | Thin progress bar (`inset-x-0 bottom-0 h-1.5`, inner `ProgressFill`) when `in_progress`. On `SeriesCard`: the share of issues finished, for a started series. |
| **Cover-wide** | Status-tinted **dimming** | Opacity reduction when archived / unresolved / fully-finished-in-rail. |
| **Ring** | **Current-position** marker | `CblWindowCard` when `isCurrent`. Other cards: unused. |

## Top-right priority cascade

Only one indicator ever renders in `top-2 right-2`. Higher priorities win:

1. **Match-status badge** — `CblIssueCard` only. Surfaces when
   `match_status !== "matched" && !== "manual"` (ambiguous / missing
   entries on the CBL detail page).
2. **State / status badge** — when the entry is in an unusual state
   (`issue.state !== "active"`, or `series.status !== "Active"`).
3. **Finished check** — green circle with `Check`, when
   `state === "active" && finished`.

The check is the shared `ReadCheck` component (`IssueCard`,
`CblIssueCard`, `SeriesCard`).

These three are mutually exclusive in every real case:

- Unmatched CBL entries have no resolved `issue`, so the finished
  check's render guard never trips.
- A finished issue is by definition `state === "active"`, so it
  can't also be archived/withdrawn.

**`SeriesCard` stacks its top-right instead.** A series is often both
finished by the reader and `Ended` by the publisher, so the cascade
would hide the read check on exactly the series a reader completed.
Its top-right is one `flex-col items-end gap-1` column, in this order:
the finished check (`ReadCheck`, label "All N issues read"), then the
publication-status badge, then the admin-only `meta` chip. The check
keeps the same corner and mark as on issue cards.

If a future card adds a fourth top-right indicator (favorite, age
rating, etc.), it joins the cascade with a documented priority — never
sit alongside another top-right element.

## Bottom-left semantics

Three card-type-specific occupants, but each card only uses one:

- **`CblIssueCard` / `CblWindowCard`** — CBL position badge (`#N`).
  Always present.
- **`SeriesCard`** — `CollectionBadge`: a 20px round chip showing
  collection ownership, with the meaning in the icon (`LibraryBig` =
  every counted issue owned, `BookMinus` = some missing) as well as
  the success / warning tone, and `role="img"` + an `aria-label`
  ("Collection complete: 5 of 5 issues"). Sits at `bottom-2.5` so it
  clears the bottom-full progress bar. Present when
  `collectionStatus(series)` returns a value AND the global
  "Collection badge" preference (`useCoverCollectionDot`, toggle in the
  `CoverSizeOptions` popover) is enabled. Readers who want pristine
  covers can hide it.
- **`IssueCard`, `ProgressIssueCard`, `OnDeckCard`** — empty today.
  Available for future indicators (downloaded badge, queue marker,
  age rating, …).

`IssueCard` historically used bottom-left for the finished check. As of
**[2026-05-21]** that moved to top-right under the cascade above; the
bottom-left slot is now intentionally free.

## Bottom-right is sacred

`QuickReadOverlay` (issue play) and `SeriesPlayOverlay` (series play
with async-resume) own `right-2 bottom-2` across every card. The
single hardest-learned lesson of the corner system: on touch devices,
`:hover` sticks after tap, so anything that lives under the play
button is invisible until the user taps elsewhere. **Never put state
indicators in bottom-right.**

The play button itself is a fixed 32×32 footprint and a hover-reveal
treatment via `group-hover:opacity-100`. Its visibility is gated by
`showActions` (only when `issue.state === "active"` or the equivalent
series state).

## Future-card checklist

When adding a new cover-card variant, walk this list before writing
the JSX:

- [ ] Top-left: does the card need a kebab? If yes, use
  `<CoverMenuButton>`. If select-mode is supported, layer
  `<SelectionCheckbox>` above it (see existing cards for the
  z-index + visibility-gate pattern).
- [ ] Top-right: pick at most one occupant per render state, following
  the priority cascade above. Add a comment block referencing this
  doc.
- [ ] Bottom-left: use a card-type-specific badge if needed; otherwise
  leave empty. Don't reuse the slot for an unrelated semantic.
- [ ] Bottom-right: drop in `<QuickReadOverlay>` /
  `<SeriesPlayOverlay>`. Don't add anything else here.
- [ ] Bottom-full: render the standard 1.5px progress bar if
  `in_progress` is meaningful for this card type.
- [ ] Dimming: if the card represents a non-vibrant state (archived /
  unresolved / fully-finished-in-rail), apply `opacity-{50,60}` on
  the cover; never on browse-first cards (`IssueCard`,
  `SeriesCard`) where vibrant covers are the point.

## Anti-patterns (don't do these)

- **Two indicators in the same corner.** If a state needs to be
  surfaced and there's already content in that corner, redesign the
  cascade — don't stack.
- **State indicator in bottom-right.** Will collide with the play
  button on touch.
- **Card-type-specific behavior in top-right beyond the cascade.**
  Top-right is shared real estate; the cascade keeps it predictable.
- **Switching slots between two cards that render the same semantic
  state.** The "I've read this" check should always be in the same
  corner regardless of which card surface you're looking at.

## Series read state

`SeriesCard` derives the viewer's read state from
`SeriesView.progress_summary` (`seriesReadState` in
`lib/series-read-state.ts`); every list endpoint fills it through
`hydrate_series`. Three states, none colour-only, all spoken:

| State | Cover | Screen reader |
|---|---|---|
| Untouched (nothing finished or in progress) | nothing | visually hidden "Unread" |
| Started | bottom-full bar, width = issues finished / total (at least 5%, so an in-progress-only series is never mistaken for untouched) | `role="img"` "3 of 5 issues read[, 1 in progress]" |
| Finished | top-right `ReadCheck` | "All 5 issues read" |

No summary on the payload, or a series with no active issues → no
state at all (never "Unread" by guess). Every addition is absolutely
positioned, so the card box — and the caption under it — is identical
in every state and matches `SeriesCardSkeleton`.
