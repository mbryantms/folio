# Linked reading lists

A master reading order (CBRO's "Marvel Master Reading Order Part #07")
runs to hundreds of issues, and the community publishes each crossover
event as its own list. Folio keeps them separate and **links** them: the
event stays one list with its own progress, and the master says where it
is read. Plan: `~/.claude/plans/linked-reading-lists-1.0.md`.

## Model

| Table | What it holds |
|---|---|
| `reading_list_links` | `parent_list_id` → `child_list_id`, `kind` `event` \| `continues`, the **anchor** (event only), `anchor_status` `placed` \| `lost`, `placement` `manual` \| `auto_twin` \| `auto_chain`, `sort_within_anchor` (several events after one issue) |
| `reading_list_breaks` | Chapter breaks: a label at an anchor, for lists whose chapters aren't marked by events |
| `reading_list_link_skips` | Per-reader "skip this event" (`user_id`, `link_id`) |
| `collection_entries` | `entry_kind = 'list'` + `cbl_list_id`: a reading list kept in a collection (third leg of the kind XOR) |

- **`event`**: the child is read after the anchor issue of the parent.
- **`continues`**: the child is the parent's next part (no anchor; it sits
  at the parent's end). One per parent.

### Anchors are keyed by issue, never by entry id

A refresh (`cbl::import::apply_parsed`) deletes and rewrites every
`cbl_entries` row, so an entry id dangles after the first refresh. An
anchor names the **issue**: its ComicVine issue id when the list carries
one, else `(series, number, volume, year)`, the same key refresh uses to
carry manual matches across. `anchor_position_hint` is a cache of where
that issue sits now.

`cbl::links::reresolve_parent` runs after every entry rewrite, in the same
transaction: the hint follows the issue, and the status turns `lost` when
the issue left the list (the row is kept, drawn at its last known
position, and turns `placed` again if a later refresh brings the issue
back). The refresh log records `links_lost` / `links_restored`.

### Rules (`api::cbl_links`)

- **Visibility**: a link may only point at a list everyone who can see
  the parent can see. A shared list links shared lists only.
- **One level**: an event list has no events of its own, and a list that
  is an event somewhere cannot take events. No `continues` loops, no
  self-links.
- **Writes** need the right to manage the parent
  (`cbl_lists::ensure_can_manage`): the owner of a personal list, an admin
  for a shared one, audited as `admin.cbl_list.link.*` /
  `admin.cbl_list.break.*`.

## Derivation (`cbl::derive`, `api::cbl_derive`)

"Find events and parts…" on a catalog list. Pure rules in
`cbl::derive`; I/O in `api::cbl_derive`.

- **Events (CBRO twin diff)**: CBRO publishes each master part twice, a
  plain list and a "With Events" twin. The preview downloads the twin and
  the candidate event lists (nothing stored), diffs the two (Myers), and
  splits each inserted run by which candidate holds each issue (the
  **smallest** candidate wins, so a roll-up never claims a specific
  event's run). Each event anchors after the plain entry before its run.
- **A run no candidate holds is reported as "unplaced", never guessed.**
  Event issues already in the plain list never move an event.
- **Parts**: `part_ordinal` reads "Part #07" / "Part 2.1"; lists in the
  same folder with the same prefix and consecutive ordinals chain.
- **Apply** imports missing event lists (with the master's ownership:
  a shared master's events import shared), one hourly import token for
  the whole apply, downloads in parallel within a 35 s budget, then links
  (`auto_twin`) and chains (`auto_chain`). A list that fails to import is
  reported and the rest still apply.

## Reading across lists (`api::cbl_link_nav`)

`cbl_link_nav` is the one resolver module; Up Next, the list page's back
link, collections and OPDS all go through it.

- **Detours**: Up Next still points at the main line and also offers
  each event the reader hasn't skipped and has something unread in
  (`NextUpView.detours`, in `event_order`). The events offered are the
  ones anchored anywhere from the current entry up to the next issue Up
  Next opens (`detours_between`), not only at the current entry: Up Next
  steps over entries that are finished, not in the library or hidden, and
  an event after one of those is still due.
- **Return**: at an event's last issue, Up Next goes back to a parent, at
  the first unread issue after the anchor (`switch_to`, `reason =
  return`). The parent is the one the reader came from
  (`?via=<view id>`), else the one with the most recent reading on its
  own issues (`parents_of`). A parent the reader has never read is not
  returned into without `via`, and a `lost` anchor is never a place to
  return to.
- **Continues**: at a list's end with a next part, Up Next moves into
  the next part's first unread issue (`reason = continues`), walking past
  parts that are finished or have nothing in the library.
- **The end of the order**: when an event ends and its parent has nothing
  left after it, Up Next follows the parent's next part; when that chain
  is empty too, the answer is `source = none` with `switch_to` naming the
  parent and **no target** ("back to the master"), never the next issue
  of the last issue's series. The end card renders that as
  `ListEndNotice`.
- **One order for events**: `cbl_link_nav::event_order` (anchor position,
  order at the anchor, age, id) sorts events on the list page, in Up
  Next, in feeds and in the export.
- **Collections**: a reading list in a collection is a detour too, never
  inlined. The collection's reading order skips list entries
  (`collection_sequence`); the list is offered after the entry before it
  (or across items Up Next steps over, `lists_between`; a list at the very
  top is offered at the first issue),
  and its end returns to the collection when `via` is that collection
  (`switch_to.collection = true`).
- `?via=` is ignored and scrubbed from the URL when stale
  (`via_param_was_stale`). It is not `?from=` because `from=start` is
  taken.

## Feeds and export

- **OPDS 1.2**: a list's acquisition feed carries `rel="related"` links
  to its events ("Event after #N label: name"), its next part
  ("Continues in: …") and its parents ("Part of: …"). A collection feed
  renders a list entry as a `subsection` entry in its place.
- **OPDS 2.0**: the same `related` links with `folio:link` /
  `folio:afterPosition` / `folio:afterIssue` properties, plus a
  `groups: [{metadata: {title: "Events in this list"}, navigation}]`
  block. A collection's list entries are navigation entries with
  `folio:entryKind = "list"`.
- **CBL export** (`GET /me/cbl-lists/{id}/export`) returns the stored
  file unchanged. `?inline_events=true` rebuilds it with each **placed**
  event's books after its anchor book, the caller's visible events only;
  lost anchors are left out rather than guessed into place.
- **Account export** (`GET /me/export`): `reading_list_links` and
  `reading_list_breaks` sections (the caller's own lists; each list named
  by id, CBL name and catalog path; anchors by issue key), and
  `entry_kind = "list"` collection entries carry a `list` ref.

## Web

- `components/cbl/links/`: `EventBand` (the event row in the master,
  `data-event-link`), `ChapterParts`, `LinkDialogs`, `DeriveLinksDialog`,
  `PartOfLink` (the back link; `?event=<link_id>` scrolls the parent to
  the band).
- Reader: `DetourButtons` on the end card / webtoon footer, `switch_to`
  handling, `lib/reader/list-context.ts` ("Back to X" / "Continue in X").
- `components/collections/ReadingListEntryCard.tsx`: the collection tile.
- **Move, reorder, repair** (`components/cbl/links/MoveEventDialog.tsx`):
  the band's "Move…" opens a picker that searches the list's entries on
  the server; "Read earlier / later here" reorders several events after
  one issue (`reorderWrites` rewrites every changed slot, so stored ties
  can't leave the order unchanged). A link or chapter break whose issue
  left the list shows in `LostAnchorsNotice` above the grid and on its
  band ("Placement lost · was after X") with "Place…"; the refresh toast
  and the History tab report how many lost or regained their place. The
  delete confirmation names the lists a list is linked into and the lists
  linked into it (`deleteLinkedNote`).
- **List page state** (`components/saved-views/CblViewDetail.tsx`):
  - *Plain list*: a per-reader, per-list switch (`usePlainListView`,
    localStorage) that renders the list with no chapters, folding, rail
    or bands. Chapters are never forced on a reader.
  - *Fold state* is keyed by `chapterKey` (the chapter's start position),
    never its index: indexes renumber when a link or break is added or
    removed elsewhere.
  - *Chapter in view*: `useCurrentChapter` lives inside the rail
    (`ChapterParts.tsx`), not the page, so crossing a chapter re-renders
    the rail only. A jump holds the highlight on its target until the
    page settles.
  - *Sticky offsets*: `usePinnedControlsHeight` publishes the pinned
    control row's height as `--pinned-controls-h`; chapter headers, the
    rail and the scroll margins on sections and bands sit at
    `--topbar-h` + that. Don't hard-code a pixel offset.
  - The entries query keeps its previous pages while a fold change moves
    its start (`keepPrevious`), and loads without chapters when the
    links request fails (an error strip offers Retry).

## Tests

- `crates/server/tests/reading_list_links.rs`: anchor re-resolution
  across refreshes.
- `crates/server/tests/reading_list_links_api.rs`: links API rules,
  breaks, derivation, Up Next detours / return / continues, collections,
  OPDS links, inline-events export.
- `crates/server/tests/perf_reading_list_links.rs`: query-count guard on
  the links GET.
- `crates/server/tests/user_export.rs`: the export sections.
