# Cover-card corner conventions

The cover cards across Folio (`IssueCard`, `SeriesCard`, `CblIssueCard`,
`CblWindowCard`, `ProgressIssueCard`, `OnDeckCard`, `UpcomingCard`) share a small set of
absolute-positioned children. To keep the reading experience consistent
across rails, grids, detail pages, and search, each corner has a fixed
purpose.

## The cover frame

Every card draws its cover inside `<CoverFrame>`
(`web/components/library/CoverFrame.tsx`). The frame is the positioned
box the slots below hang on, and it is what makes covers behave alike.
It has three parts, and a card gets all of them by using it:

| Part | Where it comes from | What it does |
|---|---|---|
| Tilt and sheen | `CoverFrame` (via `CoverTilt`) | The cover leans toward a mouse or pen pointer and a highlight follows it, when the account's "Cover tilt and sheen" effect is on and motion is not reduced. A plain box otherwise. Touch never tilts. |
| The cover box | `<Cover>` (library art) or `<UpcomingCover>` (a provider image), drawn inside the frame | The 2:3 placeholder with the rounded border, class `.cover-box`: the element the hover glow lands on. |
| The cover's colour | The server's `cover_tone` on the card's payload; `useCoverTone` (called by those two) as the fallback | The placeholder takes the cover's average colour while the image loads, and the enclosing `[data-card]` gets `--cover-tint` for its hover wash and glow. See "The cover's colour" below. |

So a card needs `data-card` on its outer element, a `<CoverFrame>`, and a
`<Cover>` (or `<UpcomingCover>`) inside it; and, for the colour on first
paint, `style={coverTintStyle(x.cover_tone)}` on that outer element and
`tone={x.cover_tone}` on the `<Cover>`. In development the frame logs
a warning when nothing inside it draws a `.cover-box`.

Two leans: `card` (the default, 7°) and `hero` (3.5°) for the large
header cover on the series and issue pages, which glows in its colour
but does not wash a card surface.

### The cover's colour

Two colours per cover: the average (the placeholder while the image
loads) and the most vibrant (the hover wash and glow, as `--cover-tint`).

The server computes both (`metadata::cover_tone`) and sends them as
`cover_tone`, twelve hex digits, on `IssueSummaryView`, `SeriesView`,
`IssueDetailView` and `UpcomingEntryView`:

- **Library covers**: the cover thumbnail job reads them from the image
  it encodes, so they follow a chosen cover page, and stores
  `issues.cover_tone`. Covers made before that get it from the
  `cover_tone` backfill (Admin → Metadata → Cover colours), which reads
  the thumbnail already on disk. A series card takes its cover issue's
  pair from the cover query it already runs.
- **Upcoming releases**: the calendar sweep reads each Metron cover once
  ([upcoming-releases.md](upcoming-releases.md), "Cover colours").

A card puts the pair into its markup with two helpers from
`web/lib/ui/cover-tone-style.ts` (not a client module, so server pages
can call them): `coverTintStyle()` on the `[data-card]` element and the
`tone` prop of `<Cover>`, which sets the box's background. Both are in
the server-rendered HTML, so a device that has never loaded the cover
shows its colour on first paint.

Without a pair (a cover not yet backfilled), `useCoverTone` falls back
to this device's cache: the browser samples a cover once it has decoded
it and remembers up to 2,500 pairs in localStorage
(`web/lib/ui/cover-tone.ts`). That only works after the first load, and
never for a cross-origin image.

**These stay still** and do not use the frame: hover previews, the
fanned cover stacks on collection cards, and row thumbnails (list rows,
search results, the release dialog).

`web/tests/dom/cover-frame.test.tsx` renders every card named in this
document's opening list with the effect on, off and under reduced
motion. It also reads that list, so a card added here without a case
there fails the test.

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

## The pill

The corners say where a signal goes. This section says what it looks
like. Every text pill drawn on cover art is a `<CoverChip>`
(`web/components/library/CoverChip.tsx`); the look is the `.cover-chip`
rule in `web/styles/globals.css`. A call site positions a chip and
caps its width. It does not restyle one.

| Property | Standard |
|---|---|
| Shape | 20px tall, `rounded-md`, 6px side padding. |
| Text | 12px, medium weight, one line; a label too long for its cover ends in an ellipsis and the full text goes in `title`. Numbers use `tabular-nums`. |
| Case | Sentence case: "Ended", "Unreadable archive". Never all caps. A raw state token goes through `coverChipLabel()`. |
| Backing | The page background at 94% opacity, with a hairline ring (14% of the foreground) and a 1px shadow. No `backdrop-filter`: the chip has to read without it. |
| Status | Text colour **and** a mark. A status tone draws its icon, or a dot when it has none, so colour is never the only signal. |
| Solid form | Errors only (`tone="error"`): something is broken or failed. |
| Round mark | Only `ReadCheck`, the 24px "read" check. A second round badge is not a chip variant. `CollectionBadge` on `SeriesCard` is the existing 20px icon badge for collection ownership; it predates this section and stays as it is. |

### Tones

| Tone | Means | Drawn as |
|---|---|---|
| `neutral` (default) | Context, not status: a position, a day, a page number, a publication status. | Foreground text. A mark only if the call site passes an icon. |
| `success` | It is done, or it is here. | Success-toned text with an icon or dot. |
| `warning` | Needs the reader's attention or a decision. | Warning-toned text with an icon or dot. |
| `info` | Something is under way or being tracked. | Info-toned text with an icon or dot. |
| `error` | Broken or failed. | The solid chip: destructive fill, its foreground label, an icon. |

The theme's status tokens are tuned for the page, and three of them miss
4.5:1 on a chip in the light or amber theme. So a chip mixes each tone
75% with the foreground, and the solid fill 80% with the foreground.
`web/tests/ui/cover-chip-contrast.test.ts` reads those numbers and the
theme tokens out of `globals.css` and checks every tone at 4.5:1 or
better in dark, light and amber, on the backing over pure black and over
pure white art.

**Not for covers:** `statusTone()`'s tinted surface (10% of the tone
under tone-coloured text), a `<Badge>` on a translucent background, a
hand-built `<span>`, palette classes such as `bg-amber-500/90`, and
`backdrop-blur` as the thing that makes text readable.

### Every pill

| Card | Corner | Pill | Tone | Wording |
|---|---|---|---|---|
| `IssueCard` | top-right | Unreadable state | `error`, `Lock` or `FileWarning` | "Password-protected", "Unreadable archive" (`issueStateLabel`); the reason in `title` |
| `SeriesCard` | top-right stack | Publication status, unless `Active` | `neutral` | The status in sentence case: "Ended", "Continuing", "Cancelled", "Hiatus" |
| `SeriesCard` | top-right stack | Metadata hint (admins) | `warning`, `TriangleAlert` | "Meta"; the name is "Meta likely incomplete — find metadata" |
| `CblIssueCard`, `CblWindowCard` | bottom-left | List position | `neutral` | "#34" |
| `CblIssueCard` | top-right | Match status, unless matched | `warning` + `CircleHelp` for "Ambiguous"; `neutral` + `CircleDashed` for "Missing" | The status in sentence case |
| `UpcomingCard` | top-right | Stage | `success` "In library", "Downloaded"; `info` "Downloading", "Tracked"; `warning` "Wanted"; `error` "Failed" | `stageBadge()` |
| `UpcomingCard` | bottom-left | Release day | `neutral` | "Wed 25" |
| Reading log card | top-right | What happened | `success` "Finished", "Series finished"; `info` "Reading session"; `neutral` "Bookmark"; each with its icon | Sentence case |
| Bookmark tile | bottom-left | Kind | `neutral` with the kind's icon | "Bookmark", "Note", "Favorite", "Highlight" |
| Bookmark tile | top-right | Favourite | `neutral`, `Star`, icon only | No text; named "Favorite" |
| Cover page picker | bottom-left | Page number | `neutral` | "12" |

The cover page picker marks the current cover with `ReadCheck` (label
"Current cover"), the one round mark.

`web/tests/ui/cover-chip-usage.test.ts` guards this. In every file that
draws on a cover it fails on a hand-built pill (a positioned element
with `backdrop-blur`, or its own rounded, padded, sized-text box), a
`<Badge>` positioned on the cover, a palette colour, and a `CoverChip`
restyled at the call site. A new card file is added to its list.

An ambiguous list entry waits on the reader's choice, so it warns. A
missing one is not in the library, which is context, not a fault: it
reads as neutral with its own icon, and the dimmed cover already says the
entry is unresolved.

## Top-right priority cascade

Only one indicator ever renders in `top-2 right-2`. Higher priorities win:

1. **Match-status badge** — `CblIssueCard` only. Surfaces when
   `match_status !== "matched" && !== "manual"` (ambiguous / missing
   entries on the CBL detail page).
2. **State / status badge** — when the entry is in an unusual state
   (`issue.state !== "active"`, or `series.status !== "Active"`).
3. **Finished check** — the accent-coloured circle with `Check`, when
   `state === "active" && finished`.

The check is the shared `ReadCheck` component (`IssueCard`,
`CblIssueCard`, `CblWindowCard`, `SeriesCard`).

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

Two more cards use the corner for their own state signal: the reading
log card says what happened ("Finished", "Reading session"), and a
bookmark tile shows a star when the marker is a favourite (it was
bottom-right, the corner reserved for play / open).

If a future card adds a fourth top-right indicator (age rating, etc.),
it joins the cascade with a documented priority — never
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
- **`UpcomingCard`** — the release day ("Wed 25") as a neutral chip.
  Always present: the day is the one thing that distinguishes cards
  inside a week.
- **Bookmark tile** (`MarkersList`) — the marker's kind ("Highlight",
  "Note") with its icon. It used to sit top-left, under the menu and
  the selection checkbox.
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

## `UpcomingCard` (upcoming releases)

A release is not in the library, so the card has no reader to open, no
menu and nothing to select. It follows the slots anyway:

- **Top-right**: the stage badge (`UpcomingStageBadge`, `onCover`
  variant: a `CoverChip` in the stage's tone, because a tinted surface
  is unreadable over cover art; a failed download is the solid chip). One state per card, by the
  server's precedence: in library → downloaded → downloading → wanted
  (or failed) → tracked. A release nobody tracks ("announced") shows no
  badge, so in "All releases" the badges mark the reader's own series.
- **Bottom-left**: the release-day chip.
- **Top-left, bottom-right, bottom-full**: empty. Bottom-right stays
  reserved for play / open even here; do not put the stage there.
- **No dimming**: this is a browse-first card.

The whole card is one button that opens the read-only release dialog.
It never carries an action on Mylar: Folio has none
([upcoming-releases.md](upcoming-releases.md)).

## Future-card checklist

When adding a new cover-card variant, walk this list before writing
the JSX:

- [ ] Cover: `data-card` on the card's outer element with
  `style={coverTintStyle(x.cover_tone)}`, the cover and its overlays
  inside `<CoverFrame>`, the art drawn by `<Cover tone={x.cover_tone}>`
  (or `<UpcomingCover>`). Never a plain `<div className="relative">`. Add
  the card to this document's opening list and to `CARDS` in
  `web/tests/dom/cover-frame.test.tsx`.
- [ ] Top-left: does the card need a kebab? If yes, use
  `<CoverMenuButton>`. If select-mode is supported, layer
  `<SelectionCheckbox>` above it (see existing cards for the
  z-index + visibility-gate pattern).
- [ ] Top-right: pick at most one occupant per render state, following
  the priority cascade above. Add a comment block referencing this
  doc.
- [ ] Pills: every text pill on the cover is a `<CoverChip>` with a tone
  from "The pill", in sentence case, and has a row in that section's
  table. The read mark is `<ReadCheck>`.
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
