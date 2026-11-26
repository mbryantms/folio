# Add to Mylar

An admin can add the series of a reading list's missing entries to a
Mylar install, and mark those issues Wanted, from the list's Resolution
tab. It is the **only** thing Folio ever asks Mylar to do, and it is off
until an admin stores a Mylar address and API key.

Everything else about Mylar stays as [upcoming-releases](upcoming-releases.md)
describes it: the snapshot is pushed to Folio and shown read-only.

## Why it is narrow

Mylar's API key is all-or-nothing: the same key can delete series and
restart Mylar. Folio holds the limit on its own side:

- [`mylar_control::client::Command`](../../crates/server/src/mylar_control/client.rs)
  is a closed enum of five commands — `getVersion`, `getComic`,
  `addComic`, `getIssueInfo`, `queueIssue` — and the only way to build a
  request. None deletes, pauses, refreshes or restarts anything.
- The key is sealed at rest (`integrations.mylar.api_key`, a secret
  setting) and never returned by any endpoint.
- Mylar takes the key as a query parameter, so the client follows no
  redirects and logs transport errors without their URL.
- Every route is `RequireAdmin`; queuing is audited as
  `admin.integration.mylar.add`.

`crates/server/tests/upcoming_view_only.rs` pins the route list, the four
Mylar settings and the command list. Widening any of them is a decision
to make on purpose, in that test.

## Settings

| Key | Meaning |
| --- | --- |
| `integrations.mylar.url` | Mylar's base URL (http or https; a sub-path is kept). Empty = off. |
| `integrations.mylar.api_key` | Mylar's API key. Secret. |

Both live, both edited on `/admin/upcoming` → Mylar → **Add to Mylar**,
with a **Test connection** button (`getVersion`). The feature is on when
both are stored. They are independent of `integrations.mylar.enabled`,
which only gates snapshot ingest.

Folio must be able to reach the address. Where a firewall keeps
everything away from Mylar, that is a rule to open (or a reverse-proxy
route that lets `/api` through); the snapshot path needs neither.

## Flow

1. `POST /api/admin/integrations/mylar/requests`
   `{cbl_list_id, entry_ids?, want_issues}` — `entry_ids` absent means
   every missing entry of the list; at most 500 ids otherwise.
   [`requests::create`](../../crates/server/src/mylar_control/requests.rs)
   reads the missing entries' ComicVine ids and writes one
   `mylar_add_request` row per series (skipping series the last snapshot
   already shows on the watchlist) and, with `want_issues`, one per issue
   (skipping issues the snapshot shows as wanted). No Mylar call happens
   here. Entries without a ComicVine series id are counted and left out.
2. [`sweep::run`](../../crates/server/src/mylar_control/sweep.rs) runs
   once a minute, and once right after a request. Per tick, at most 40
   rows, series rows first:
   - series: `getComic`; if Mylar lacks it, `addComic`. Mylar only
     queues the import, so the row is `done` as soon as Mylar accepts.
   - issue: `getIssueInfo`. Mylar knows an issue only after importing
     its series, so "not found" waits with backoff (2, 4, 8, 16, then
     every 30 minutes) and fails after 24 hours. A known issue that is
     not already Wanted / Snatched / Downloaded / Archived gets
     `queueIssue`.
   - Mylar unreachable or answering 5xx ends the tick and counts against
     no row. A refusal fails the row with Mylar's reason.
3. `GET /api/admin/integrations/mylar/requests?cbl_list_id=` returns each
   missing entry's series and issue state — `in_mylar`, `requested` or
   `failed` — from the snapshot tables and the request rows. Targets
   Folio knows nothing about are left out.

One row per target (`UNIQUE NULLS NOT DISTINCT (cv_series_id,
cv_issue_id)`): asking again resets a `failed` row to `pending`, and a
`done` row only once it is a day old (by then a snapshot normally shows
the series in Mylar; if not, it was probably removed there). A `pending`
or freshly `done` row is left alone and counted as already requested.

## Web

[`MylarAdd.tsx`](../../web/components/cbl/MylarAdd.tsx) adds, to the
Resolution tab of a list with missing entries, for an admin with Mylar
configured: a bar (Select, Add selected, Add all missing), a button and
state badge on each missing row, and one confirm dialog with the "Also
mark these issues Wanted" choice. State logic is in
[`lib/cbl/mylar.ts`](../../web/lib/cbl/mylar.ts).

## Reviewer heuristic — reject PRs that

- add a `Command` variant, or build a Mylar request any other way;
- return, log or export the API key, or put it in a URL that is logged;
- call Mylar from a request handler other than the connection test (the
  sweep is the caller: it paces the adds and survives Mylar being down);
- add an add-to-Mylar control for non-admins, or on the Upcoming page
  (its Mylar data stays badges and text).
