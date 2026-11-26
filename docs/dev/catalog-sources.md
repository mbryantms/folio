# Catalog sources

The community catalog (`DieselTech/CBL-ReadingLists`, the default
`catalog_sources` row) holds 1,713 `.cbl` files from several
reading-order projects, each laid out its own way. `cbl::sources`
(`crates/server/src/cbl/sources.rs`) is the one module that reads a
catalog path: it says which project a file came from and which files
make up one order. Nothing else parses catalog paths.

## What is in the catalog (surveyed 2026-11-25)

| Source | Where | Files | Shape |
|---|---|---|---|
| CBRO (Comic Book Reading Orders) | `<Pub>/Master Reading Order/CBRO/` | Marvel 20, DC 17 | plain master parts, `… Part #07 (WEB-CBRO)`; events are separate lists |
| CBRO, with events | `<Pub>/Master Reading Order/CBRO - With Events/` | Marvel 20, DC 17 | the same parts with every event's issues inline |
| CBRO events | `<Pub>/Events/CBRO/<era>/` | Marvel 158, DC 124 | one list per event, grouped by era |
| CMRO (Complete Marvel Reading Order) | `Marvel/Master Reading Order/CMRO/{Essential,Core,Main,Expanded}/` | 7, 25, 31, 42 (+14 in `Misc/`) | one order at four depths, cut into 1,000-issue files, `… Reading Order-Part 01`; events inline, no twins |
| Marvel Guides | `<Pub>/Master Reading Order/Marvel Guides/<era>/<Part NN …>/` | Marvel 326, DC 16 | an era is one order of named sub-chapters, `… Part 2.1 Age of Ultron (MG)`, in a sub-folder per part |
| UXRO (Ultimate X-Men Reading Order) | `Marvel/Teams/X-Men/Ultimate X-Men Reading Order/` | 31 | era files ordered by a leading `[1963-1974]` |
| LoCG, Official, CBH, CBT | `<Pub>/Events/…` | about 300 | standalone event lists |
| "with events" character / team lists | outside the master folders | 45 | events inline |

CMRO, from a sample of eight files (parts 1 and 2 of each depth): every
book carries a ComicVine issue id, so matching is by id; no book repeats
inside a file; files hold 994 to 1,000 books. The largest file in the
catalog is 415 KB (the blob cap is 4 MB) and none is near the
5,000-entry cap. Not checked across all 119 CMRO files: ids and duplicates per book, and
whether each depth contains the one below it (see "The audit").

## What a profile says

`sources::profile(path)`:

- `source`: `cbro`, `cbro_with_events`, `cmro`, `marvel_guides`, `uxro`,
  `locg`, `official` or `other`.
- `order_key` + `part`: the files of one order share a key and sort by
  `part`. The key is the folder plus the file-name prefix before
  "Part N"; for Marvel Guides it is the era folder, whichever part
  sub-folder a file sits in. UXRO has no part numbers; its files sort by
  the years of their `[1963-1974]` tag (start, then end).
- `depth`: CMRO only.
- `plain_twin`: for a with-events file, the plain file it mirrors.

`part_of` finds the first stand-alone "Part" followed by a number, so a
title containing the letters ("Departed") neither hides nor fakes a part
number. `derive::part_ordinal` is the same function.

The catalog listing (`GET /catalog/sources/{id}/lists`) carries these
fields on every entry; the web's catalog browser labels each file's
source and groups an order's files into one "Import all N parts" row.

## Importing a whole order

`POST /me/cbl-lists/import-order` (`api::cbl_orders`) takes any one file
of an order, imports the parts the caller doesn't have yet, in reading
order, and chains every adjacent pair (`continues`, `auto_chain`).

- One call stops after 35 s of imports and reports `remaining`; the web
  calls again until it is zero. Imported parts are reused, so a repeat
  only does what is left.
- One call spends one hourly import token. Each new list still counts
  against the per-user list cap; hitting it stops the run and says so.
- A part that fails to import leaves its neighbours unchained until a
  later run imports it.

## The audit

`crates/server/tests/catalog_sources_snapshot.rs` runs the rules over a
snapshot of every `.cbl` path in the catalog
(`crates/server/tests/fixtures/catalog-paths.txt`, 1,713 files) and pins what
they say: the count per source, CMRO's four depths numbered 1..N with no
gaps, a plain twin for each of the 37 with-events masters, each Marvel
Guides era as one order (its files sit in a sub-folder per part), UXRO in
era order, and the three orders where upstream gives two files the same
number (they read in file-name order).

`just catalog-snapshot` refreshes the snapshot and re-runs it; what fails
is what changed upstream, or a layout the rules misread. Writing it found
two such misreadings before anything shipped: an era split into dozens of
three-file orders, and X-Men eras that share a start year.

Known and left alone: the "Spider-Man Complete 616 Chronology" folder
numbers its files with a leading `NN -` and restarts "Part N" per arc, so
its files are not grouped as one order.

What the audit does not cover: file contents (ComicVine ids per book,
duplicates, whether each CMRO depth contains the one below it). That
needs the files, not just their paths.

## Adding a source

Teach `profile` to recognise it (a path segment or a file-name tag),
give its files an `order_key` / `part` if they form an order, add its
paths to the tests in `sources.rs`, and add a row above.
