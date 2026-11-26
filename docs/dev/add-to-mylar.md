# Add to Mylar

An admin can add the series of a reading list's missing entries to a
Mylar install, and mark those issues Wanted, from the list's Resolution
tab. A few more admin actions keep Mylar's wanted list honest: check it
against the library, search now, recheck a series' files, refresh a
series, and un-want an issue.

This module is the **only** place Folio calls Mylar, and it is off until
an admin stores a Mylar address and API key. Everything else about Mylar
stays as [upcoming-releases](upcoming-releases.md) describes it: the
snapshot is pushed to Folio and shown read-only.

## Why it is narrow

Mylar's API key is all-or-nothing: the same key can delete series and
restart Mylar. Folio holds the limit on its own side:

- [`mylar_control::client::Command`](../../crates/server/src/mylar_control/client.rs)
  is a closed enum and the only way to build a request. Ten commands:
  `getVersion`, `getComic`, `addComic`, `getIssueInfo`, `queueIssue`,
  `unqueueIssue`, `getWanted`, `forceSearch`, `recheckFiles`,
  `refreshComic`. None deletes a series, reconfigures Mylar or stops it.
  Deliberately left out: `delComic`, `changeStatus` (it can rewrite every
  series at once), pause / resume, the provider commands, `forceProcess`,
  `shutdown`, `restart`, `update`. Mylar's API has no rename command.
- The key is sealed at rest (`integrations.mylar.api_key`, a secret
  setting) and never returned by any endpoint.
- Mylar takes the key as a query parameter, so the client follows no
  redirects and logs transport errors without their URL.
- Every route is `RequireAdmin`; each action that changes something is
  audited (`admin.integration.mylar.{add,search,recheck,refresh,unwant}`).

`crates/server/tests/upcoming_view_only.rs` pins the route list, the four
Mylar settings and the command list (and names the left-out commands). Widening any of them is a decision
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

## Adding from the Upcoming page

`POST /api/admin/integrations/mylar/series` `{cv_series_id, series_name}`
queues one series (`requests::create_series`) under the same rules as a
reading-list add: nothing for a series the snapshot shows on the
watchlist, or one with a request waiting or just accepted. The sweep
carries it out. It adds the series only; which issues Mylar then fetches
is Mylar's own setting.

The Upcoming release dialog shows **Add series to Mylar** when
`UpcomingSummaryView.can_add_to_mylar` (an admin, with the address and
key stored), the entry has a `cv_series_id`, and it is not `tracked`. An
entry Metron has not linked to ComicVine says so instead. It is the only
control on the Upcoming page that acts on Mylar.

## Wanted-list actions

Each is one admin click and makes its one or two Mylar calls inline.
A Mylar fault answers 502 with Mylar's reason.

| Route (under `/api/admin/integrations/mylar`) | What it does |
| --- | --- |
| `GET /wanted-check` | `getWanted`, then [`wanted::check`](../../crates/server/src/mylar_control/wanted.rs): which wanted issues a live library issue already carries the ComicVine id of (grouped by series, first 200 series, totals for all), and how many distinct reading-list missing issues Mylar is not looking for. Read-only. |
| `POST /search` | `forceSearch`: Mylar searches its whole wanted list. Mylar answers only when the search ends, so Folio waits 8 s and reports `started`. |
| `POST /series/{cv_series_id}/recheck` | `recheckFiles`: Mylar rescans the series' folder. `done`, or `started` on a timeout. |
| `POST /series/{cv_series_id}/refresh` | `refreshComic`: Mylar queues a re-read from ComicVine. Always `started`. |
| `POST /issues/{cv_issue_id}/unwant` | `unqueueIssue`: a Wanted issue goes back to Skipped. Also drops Folio's request row and the stored snapshot's line for it. |

Mylar quirks the handlers guard against (read from Mylar's `api.py`,
2026-11):

- `recheckFiles` and `refreshComic` do nothing useful for a series Mylar
  lacks, so both check `getComic` first and answer 404.
- `refreshComic` queues the refresh and then fails formatting its own
  success message, which arrives as a 5xx. After the `getComic` check a
  5xx is read as queued.
- `unqueueIssue` writes `Skipped` for any id, known or not, and would
  demote a Downloaded issue. Folio calls it only when `getIssueInfo`
  says `Wanted` (404 for unknown, 409 for any other status).

## Web

[`MylarAdd.tsx`](../../web/components/cbl/MylarAdd.tsx) adds, to the
Resolution tab of a list with missing entries, for an admin with Mylar
configured: a bar (Select, Add selected, Add all missing), a button and
state badge on each missing row, and one confirm dialog with the "Also
mark these issues Wanted" choice. State logic is in
[`lib/cbl/mylar.ts`](../../web/lib/cbl/mylar.ts). A row whose series
Mylar has also gets a ⋯ menu: Refresh series, Recheck files, and
Un-want for a wanted issue.

[`MylarToolsCard.tsx`](../../web/components/admin/upcoming/MylarToolsCard.tsx)
on `/admin/upcoming` holds **Check wanted list** (fetched only on click)
and **Search now**, with Recheck files / Refresh per series and Un-want
per issue in the check's result.

## Reviewer heuristic — reject PRs that

- add a `Command` variant, or build a Mylar request any other way;
- return, log or export the API key, or put it in a URL that is logged;
- call Mylar from a request handler for anything that can be many calls
  (adds go through the sweep: it paces them and survives Mylar being
  down); a handler makes one admin click's one or two calls, no loops;
- run a Mylar call on a timer or on page load (the wanted check is
  fetched on click only);
- add an add-to-Mylar control for non-admins, or on the Upcoming page
  (its Mylar data stays badges and text).
