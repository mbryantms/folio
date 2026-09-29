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
sticky-finished and bulk-mark tests. Web: `web/tests/dom/progress-lifecycle.test.tsx`
and `web/tests/reader/progress-writer.test.ts`.
