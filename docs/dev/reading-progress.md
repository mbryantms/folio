# Reading progress: storage and the cross-device rule

Reading position lives in `progress_records` (one row per `(user, issue)`),
written by `POST /progress` from the web reader and by the OPDS
Progression, KOReader, and Komga-shim endpoints through the same helper
(`crates/server/src/api/progress.rs::upsert_for_run`). This page records
the conflict rule those writers share (roadmap WP-1.3; audit UX-1).

## Columns that matter

| Column | Meaning |
|---|---|
| `last_page` | 0-based resume page |
| `percent` | `last_page / page_count` |
| `finished`, `finished_at` | sticky completion flag and when it flipped |
| `run` | reading-run counter; 0 is the first read |
| `is_backfill` | catalog/sync write, not active reading (excluded from activity stats) |
| `page_hash` | hash of the page image at `last_page` when it was written (WP-6.2); NULL for bulk marks and pre-WP-6.2 rows |

## The reading-run rule

Every write is either **implicit** (the reader's debounced per-page write,
`finished` omitted) or **explicit** (`finished: true|false`: mark read, mark
unread, the last-page auto-finish).

Within one run:

- an implicit write only moves forward: `last_page = max(stored, page)`. A
  stale debounced write from a second device can never drag the position
  backwards;
- an explicit write stores `page` as given. "Mark as unread" therefore
  resets the floor to 0 without changing the run;
- `finished` is sticky on implicit writes: jumping back to a bookmarked page
  never un-finishes an issue.

Opening a new run (`restart: true` on the write):

- bumps `run`, stores `page` (normally 0), clears `finished` and
  `finished_at`. The reader sends it on **Read from beginning** and when a
  **finished issue is reopened from the cover** (otherwise the floor would
  swallow the re-read's first page turns);
- a write tagged with a `run` older than the stored one is ignored and the
  stored record is returned. A device left open on the previous read
  therefore cannot regress the new one, and it silently follows the new
  run the next time it opens the issue (the reader reads `run` from
  `GET /progress?issue_id=…` and echoes it on every write, adopting the
  value from each reply).

Clients that do not send `run` (OPDS, KOReader, Komga shim) write into the
current run and are subject to the same floor.

## Page anchoring: when the archive changes

Progress and markers (`markers.page_index`) are both `(issue, page)`
ordinals, and both are re-anchored by one helper,
[`reading::page_remap::reanchor_issue`](../../crates/server/src/reading/page_remap.rs),
whenever the archive's page list changes under a stable issue id.

**Capture (WP-6.2).** Every single-issue progress write (`upsert_for_run`:
the reader, OPDS Progression, KOReader, Komga) and every marker create
(`POST /me/markers`; markers have no other server-side write path) stores
`page_hash`: hex BLAKE3 of the page entry's decompressed bytes
([`reading::page_hash`](../../crates/server/src/reading/page_hash.rs)),
read through the same `zip_lru` reader the page server streams from, so
it hashes the image the user was actually shown. Entry names, compression
and container format are not part of the hash. An implicit write that
keeps the stored page reuses the stored hash rather than re-reading it.
Capture is soft: a page that can't be read (CBR/CB7 without conversion,
unreadable archive) stores NULL. Bulk mark read/unread (series, matching,
CBL, multi-select) stores NULL by design: those rows anchor "first page"
or "last page", which the ordinal fallback already preserves.

**Two authorities.**

| Trigger | Authority | Behaviour |
|---|---|---|
| Folio's page editor (`jobs::archive_edit`) | `Ordinal` | The edit's own op simulation is the exact old→new map (WP-1.2). Every anchor is then re-stamped with the hash of the image now at its ordinal — including pre-WP-6.2 rows, and including non-structural edits (a rotated page hashes differently; without the re-stamp the follow-up rescan would read its markers as drifted). Hashing runs only when the issue has anchors. |
| Rescan finding new bytes (`scanner::process`, `content_changed`) | `Hash` | Anchors with a `page_hash` move to the page holding that image (nearest copy when the image repeats, e.g. blank pages). Anchors without a hash use the ordinal guess: identity, or truncation when the page count shrank. An anchor whose image is gone also uses the ordinal guess. The new archive is hashed only when some anchor on the issue has a hash. |

**Drift notes on markers.** A marker whose page was removed lands on the
nearest surviving page with the `page-removed` tag (WP-1.2). A marker
whose image is gone from a replaced archive while its ordinal survived
keeps the ordinal and gains `page-drift`. It also keeps its old hash, so
if a later replacement restores the image the next rescan moves it back
and clears the tag. Legacy (NULL-hash) anchors are not stamped by a
rescan, because stamping the image at a guessed ordinal would make the
guess permanent. They pick up a hash on their next progress write or
archive edit, or from the lazy backfill below.

**Lazy backfill (WP-8.4).** Anchors written before WP-6.2 get their hash
the next time the issue is opened for reading, not from a library-wide
job. [`reading::page_hash_backfill`](../../crates/server/src/reading/page_hash_backfill.rs)
runs when the page server (`GET /issues/{id}/pages/{n}`, including the
`?w=` variant path on a cache miss) or the OPDS-PSE streamer opens the
issue's archive in the shared `zip_lru` cache (a cache miss, so once per
open, not per page), and when the reader's per-issue marker fetch
(`GET /me/issues/{id}/markers`) returns a marker without a hash — the
reader may serve every page from cached width variants without ever
opening the archive. The work is spawned off the request path, one task
per issue at a time (an in-flight set on `AppState`). Each pass:

- finds the page ordinals with an unhashed marker or progress row (one
  query; `markers(issue_id, page_index)` and the partial
  `progress_records_issue_unhashed_idx` from `m20270604`), ordinals past
  the page count excluded, at most `MAX_PAGES_PER_OPEN` = 32 of them;
- hashes each page through the cached reader, releasing its lock between
  pages so page serving isn't starved;
- stamps every unhashed row on that page with `… AND page_hash IS NULL`
  (a concurrent capture or re-anchor wins) and **without** bumping
  `updated_at`.

An issue with more pending pages finishes on later opens. The stamp
records the page the anchor points at now, i.e. the page the reader
shows for it. Markers tagged `page-removed` are skipped: their ordinal is
a guessed neighbour of a page that no longer exists.

Restored markers (`POST /me/markers/restore`, the Undo of a delete) keep
the `page_hash` the client snapshotted; a snapshot without one is filled
by the same backfill.

The account export (`GET /me/export`) carries `page_hash` on every
progress row and marker.

Re-anchoring moves bump `updated_at`. A hash-only re-stamp does not, so
it doesn't wake `GET /progress?since=` sync clients. Progress `percent`
is re-based on the new page count. Progress has no tags. A drifted
resume position just falls back to its ordinal.

**Not affected:** the OCR cache key stays `content_hash` + ordinal
(`ocr::cache::cache_key`). A content change already invalidates it.

## What this deliberately changes

Before WP-1.3 the last write won. A jump back through the page strip used
to move the resume point backwards; it no longer does (the furthest page
wins). Use **Read from beginning** for a re-read, or **Mark as unread** to
reset the position.

## Tests

`crates/server/tests/progress.rs`: `implicit_write_never_regresses_within_a_run`,
`explicit_writes_bypass_the_floor`, `restart_opens_a_new_run_and_clears_finished`,
`stale_run_write_from_another_device_is_ignored`,
`legacy_client_without_run_writes_into_the_current_run`, plus the older
sticky-finished and bulk-mark tests. Anchoring:
`crates/server/tests/page_hash_anchoring.rs` (capture, reordered
replacement, ordinal fallback for legacy rows, drift note + recovery,
editor re-stamp), `page_hash_backfill.rs` (lazy backfill: stamping,
the per-open bound, the page-server and marker-fetch triggers, export), `markers_archive_edit.rs` (WP-1.2 edit map), and the
`reading::page_remap` unit tests. Web: `web/tests/dom/progress-lifecycle.test.tsx`
and `web/tests/reader/progress-writer.test.ts`.
