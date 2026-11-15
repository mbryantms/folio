# Upcoming releases

What is coming out in the next weeks, shown read-only. Two sources feed
the feature and Folio writes back to neither.

| Source | What it gives | How it arrives | Status |
| --- | --- | --- | --- |
| **Metron** (the release calendar) | Every publisher's issues by store date, about six weeks ahead | A scheduled Folio job lists Metron's API | built |
| **Mylar** (the tracking window) | The current week's pull list with each issue's status, the watchlist, the download queue | A script on the Mylar host pushes a snapshot **to** Folio | ingest built; shown to readers in a later change |

Plan: `~/.claude/plans/upcoming-releases-1.0.md`.

## The view-only invariant

Folio never controls anything outside itself through this feature.

- The calendar job only **reads** Metron (`GET /api/issue/`,
  `GET /api/series/{id}/`).
- There is no Mylar address or API key setting, and there must never be
  one. Mylar's API key is all-or-nothing (it can delete series and
  restart Mylar), and many installs run Mylar with its login off behind a
  firewall. Mylar data reaches Folio only by push.
- The admin page and (later) the Upcoming page show state. No button
  orders, downloads, searches or marks anything in another system.

**Reviewer heuristic — reject PRs that:**

- add a setting, env var or config field that holds a Mylar URL, host or
  key;
- add an HTTP client call to anything under `crate::upcoming` other than
  through `MetronClient`;
- add a UI control whose effect is outside Folio.

## Why Metron, and how far ahead

Measured 2026-11-14 against the live APIs:

| Provider | Issues listed per future week |
| --- | --- |
| Metron | 64–84 for weeks +1 to +6 (the current week had 84), 30 and 43 for +7 and +8, single digits after |
| ComicVine | 0 for every future week (121 for the current week) |

So the calendar reads Metron only, with a default window of eight weeks
(`upcoming.weeks_ahead`, 1–12). Metron lists fewer small-publisher titles
than a full pull list (84 against 102 for the same week).

Metron's list rows carry the issue id, number, store date, cover date,
cover image and `series {id, name, volume, year_began}`. They do **not**
carry the publisher or the ComicVine volume id, which is the id Folio's
`external_ids` and Mylar's watchlist share. Those come from one
series-detail request per series.

## Tables (migration `m20271118_000001_release_calendar`)

| Table | Row | Notes |
| --- | --- | --- |
| `release_calendar_issue` | one Metron issue with a store date in the window | `last_seen_at` is stamped by every pass that returns it |
| `release_calendar_series` | one Metron series with an issue in the window | `publisher`, `series_type`, `cv_series_id` are null until the detail fetch; `detail_fetched_at` records it |
| `release_calendar_state` | one row, `id = 1` | the pass's resume point, last success, last failure and backoff |

## The list pass

`upcoming::calendar::list_pass`, run as the `release_calendar` queue job
(`jobs/release_calendar.rs`, concurrency 1).

- **Window**: Monday of two weeks ago through the Sunday `weeks_ahead`
  weeks after the current week (`upcoming::window`). Weeks are
  Monday–Sunday everywhere in this feature.
- **One request per week** (`MetronClient::list_issues_by_store_date`),
  a second only when a week overflows a page. Rows are deduped on the
  Metron issue id and upserted; a series row keeps its detail columns.
- **Resumable**: the next week to list is saved after every week. A pass
  stopped by the budget gate or by a `429` returns `Paused` and the next
  run continues at that week, as the same pass (same `pass_started_at`).
  A stopped pass older than 24 h, or one whose window no longer matches
  the settings, starts over.
- **Removal**: only a pass that listed every week to its last page
  deletes anything. It deletes window rows it did not stamp (Metron no
  longer lists them). A release that moved to another week inside the
  window was stamped under its new date and stays.
- **Pruning**: rows whose store date is more than 28 days past, then
  series left without an issue.
- **Failure**: a non-quota error records `last_error` and a 30-minute
  backoff (`next_attempt_at`) and deletes nothing. The handler returns
  `Ok`: an apalis retry would run straight into the same failure and
  spend the same budget. The sweep queues the next attempt once the
  backoff has run out.

**Who queues it.** Always through `jobs::release_calendar::enqueue`,
whose Redis `SET NX` key is held from the push until the pass returns,
so at most one is queued or running and a trigger that arrives mid-pass
adds nothing:

| Trigger | When |
| --- | --- |
| daily cron | 05:15 UTC |
| enrich sweep | a pass is stopped part-way, or the last complete one is older than 26 h |
| admin "Sync now" | `POST /admin/upcoming/sync` (also clears a failure backoff) |

## The enrich sweep

`upcoming::calendar::enrich`, a cron closure every five minutes
(`jobs::release_calendar::sweep`) with a Redis overlap lock. It stands
aside while a list pass is in flight or stopped part-way: both draw on
the same per-minute budget, and the listing goes first. Up to 10
series per tick, never-fetched first, get their detail
(`fetch_series_cached`, which also warms `metadata_cache`). A series
still without a ComicVine id is asked again after 3 days while it has a
release today or later, with an uncached request: a new #1 often gains
the id after it is announced. A failed fetch is stamped too, so one bad
series cannot hold the head of the queue.

While a tick is fetching it holds a `release_calendar_enrich` background
task, so it shows on **Admin → Background work**.

## Metron budget

Nothing reserves Metron headroom for interactive matching: every caller
draws from the same two Redis buckets (20 per minute, 5,000 per day). So
this background caller holds itself back. Before **every** request it
reads the buckets (`rate_limit::snapshot`) and stops when 8 or fewer
per-minute, or 1,000 or fewer per-day, requests remain. A stop is a
pause, not a failure.

A normal pass is eleven requests, which fits a fresh minute bucket with
the reserve intact. A wider window pauses once and the sweep resumes it
within five minutes.

## Settings

All four are database-only (`app_setting`), live, with no env variable
and no bootstrap row.

| Key | Default | Effect |
| --- | --- | --- |
| `upcoming.enabled` | `false` | Master switch for the jobs (and, later, the Upcoming page) |
| `upcoming.weeks_ahead` | `8` (1–12) | The window's far edge |
| `integrations.mylar.enabled` | `false` | Accept pushed Mylar snapshots (needs `upcoming.enabled` too) |
| `integrations.mylar.stale_after_minutes` | `180` (30–10080) | When a snapshot is shown as stale |

The calendar also needs Metron enabled with credentials
(`metadata.metron.*`); with either missing, the jobs do nothing and the
admin card says so. Turning a switch off deletes nothing.

## Admin surface

`/admin/upcoming`: the two calendar settings, a status card (rows,
per-week counts, last full sync, a paused or failed pass, series waiting
for details) with **Sync now**, and the Mylar card (the switch, the
stale threshold, the ingest token, and the last snapshot's time and
counts).

| Endpoint | |
| --- | --- |
| `GET /api/admin/upcoming/status` | read-only; in the audit-check allowlist |
| `POST /api/admin/upcoming/sync` | 202; audited as `admin.upcoming.sync`; 409 while the calendar cannot run |

## The Mylar snapshot

Operator guide: [`docs/install/mylar.md`](../install/mylar.md). The
script, its unit files and its tests live in
[`integrations/mylar/`](../../integrations/mylar/), versioned with the
endpoint they post to.

### Why push, and why a script

Measured on a real install (2026-11-14): Mylar's API key is
all-or-nothing, the install ran with Mylar's login off behind a host
firewall, and the API exposes less than the database (no download queue,
no job schedule). A pull from Folio would have needed a firewall opening
that hands the Folio host Mylar's whole login-less UI. So the direction
is reversed: a script on the Mylar host reads `mylar.db` with `mode=ro`
and POSTs to Folio. It never calls Mylar's HTTP API and never reads
`config.ini`.

Mylar only knows the **current** week (its pull list is loaded week by
week; it holds no future-dated issue). The watchlist is what reaches
further: any calendar entry whose ComicVine volume id is on it is
"tracked", whichever week it is in.

### Snapshot format (schema version 1)

`upcoming::mylar::MylarSnapshot`. Blocks: `meta`, `pull_list`,
`watchlist` (rows of `[cv volume id, status, still publishing]`),
`wanted`, `history`, `queue {counts, items}`, `jobs`. Unknown fields are
ignored, so a script may add to a version; a different `schema_version`
is refused with 422 naming the version the server accepts. The caps
(`MAX_PULL` 2,000, `MAX_WATCH` 50,000, …) are the same constants in the
script and the server. A real snapshot (4,319 watched series) is about
176 KB; the route keeps the 2 MiB group body limit.

### `POST /integrations/mylar/snapshot`

In the `bare` group (an external client calls it), alone in
`api/mylar_ingest.rs`, with its own per-IP bucket (`MYLAR_INGEST`) and a
30 s timeout (the bare group has none).

- **Auth before body**: `MylarIngestAuth` is a parts extractor ahead of
  `Validated<MylarSnapshot>`. Integration off (`upcoming.enabled` and
  `integrations.mylar.enabled` must both be on) → 403; a Bearer that is
  not the token → 401 with `WWW-Authenticate: Bearer`. Neither parses the
  body. A POST with no Bearer at all is stopped earlier by the CSRF
  middleware (403 `auth.csrf`); the route is deliberately not in
  `CSRF_EXEMPT_PATHS`.
- **Token**: `fmi_` + 52 base32 characters (256 random bits), stored as a
  SHA-256 digest in `integration_tokens` (`name = 'mylar'`), compared in
  constant time. It is not a user and opens nothing else. Created or
  replaced by `POST /api/admin/integrations/mylar/token` (the plaintext
  is returned once), revoked by `DELETE …/token` (`?purge=true` also
  deletes the stored snapshot). Both are audited
  (`admin.integration.mylar.token.{create,revoke}`).
- **No failed-auth lockout**: behind a reverse proxy every caller shares
  one address, so a lockout would let anyone block the pusher.
- **No audit row per push**: `audit_log` needs a user actor. A push is
  recorded on `mylar_snapshot.received_at` and
  `integration_tokens.last_used_at` / `last_used_ip`.

### Storage (migration `m20271119_000001_mylar_snapshot`)

`upcoming::mylar::store` replaces the previous snapshot in one
transaction: `mylar_watch`, `mylar_pull` (surrogate key: a second
printing repeats series + issue), `mylar_wanted`, and the single
`mylar_snapshot` header row, which also carries `queue`, `jobs` and
`history` as JSON for the admin panel. A push whose content hash equals
the stored one is a **heartbeat**: only `received_at` and `generated_at`
move.

### Freshness

`upcoming::mylar::freshness`, judged by Folio's `received_at`, never the
sender's clock: `fresh` up to `integrations.mylar.stale_after_minutes`,
then `stale` (admins are warned), then `expired` after 7 days (no longer
shown to readers).

## Metrics

`folio_release_calendar_issues` and
`folio_release_calendar_last_sync_timestamp_seconds`, both set when a
pass finishes; `folio_mylar_snapshot_received_timestamp_seconds`, set
when a snapshot is accepted ([metrics.md](metrics.md)).

## Tests

`crates/server/tests/release_calendar.rs` (wiremock Metron): disabled is
silent; listing, paging and dedupe; budget pause and resume; removal,
moved releases and pruning; failure backoff; quota pause; enrich and the
re-check; the sweep's queueing; the admin endpoints' auth and audit.
`crates/server/tests/mylar_ingest.rs`: off is 403 even with the token; a
wrong token is refused before the body is read; store, replace and
heartbeat; bad and oversized bodies; the token endpoints; freshness.
`integrations/mylar/test_folio_mylar_push.py` (CI job "Mylar push
script — tests"): the snapshot's content, that nothing sensitive leaves
the host, change detection and the heartbeat, failures.
`web/tests/admin/upcoming-admin.test.tsx` covers the admin page.
