# Metadata providers — operator guide

For the architecture + developer reference, see
[`metadata-providers.md`](metadata-providers.md). This document is
about the knobs you can turn as an operator + how to recover when
things misbehave.

## Quick start — getting matches flowing

1. **Get API credentials.**
   - **ComicVine**: free; register at <https://comicvine.gamespot.com/api/> and copy the API key
     from your profile. Rate limit: 200 requests/hour, max 1
     request/second (Folio honors both via the per-provider token
     bucket).
   - **Metron**: free; create an account at <https://metron.cloud/>,
     then generate a token under **API Tokens** on your account page
     and paste it as the *API token*. Username + password (HTTP
     Basic) still work as a fallback but Metron is phasing Basic auth
     out (see [Metron token auth](#metron-token-auth-limits-and-the-budget-bar)).
     Rate limit: 20 requests/minute (burst) + 5,000 requests/day
     (sustained; higher for OpenCollective supporters).
   - **Grand Comics Database (GCD)**: free; register at
     <https://www.comics.org/> and enter the account's username +
     password. Strongest on Golden/Silver Age and non-US runs. Rate
     limit: 2,000 requests/day with an account (30/hour anonymous);
     Folio paces itself at ~100/hour and 1 request/second (see
     [GCD](#grand-comics-database-gcd)).

2. **Plug them in.** `/admin/metadata` → **Providers** tab. Paste
   the credentials + flip the master toggle on. The "Test" button
   makes a round-trip against the provider's health endpoint and
   surfaces the actual quota remaining.

3. **Fetch metadata on a series.** Navigate to a series page →
   Actions menu → **Fetch metadata**. The dialog runs a search
   across every enabled+configured provider, ranks results, and
   shows them with a HIGH/MEDIUM/LOW confidence badge. Click
   **Preview** on a candidate to see the per-field diff, opt in
   to the fields you want to apply, and **Apply**.

The same flow works at the issue level: open an issue → Actions
menu → Fetch metadata.

## Settings reference

Every setting lives in the `app_setting` table + can be edited
through `/admin/metadata` → **Settings** tab (or via
`PATCH /api/admin/settings` for scripted setups).

### Provider credentials (`/admin/metadata` → Providers)

| Setting | Type | Default | Notes |
|---|---|---|---|
| `metadata.comicvine.api_key` | secret | — | AEAD-sealed at rest. Trim whitespace on paste (CV rejects keys with trailing newlines as "Invalid API Key"). |
| `metadata.comicvine.enabled` | bool | false | Master toggle. Search + apply skip CV when off. |
| `metadata.metron.api_token` | secret | — | **Preferred.** `Authorization: Bearer <token>`; generated under *API Tokens* on the metron.cloud account page. AEAD-sealed at rest; trimmed on save. When set, the username/password pair is ignored. |
| `metadata.metron.username` | string | — | HTTP Basic username — fallback when no token is set. |
| `metadata.metron.password` | secret | — | AEAD-sealed at rest. Fallback with the username. |
| `metadata.metron.enabled` | bool | false | Master toggle. |
| `metadata.gcd.username` | string | — | comics.org account username (HTTP Basic). |
| `metadata.gcd.password` | secret | — | AEAD-sealed at rest; trimmed on save. Both username and password are required. |
| `metadata.gcd.enabled` | bool | false | Master toggle. GCD runs last in priority (after Metron and ComicVine). |

`COMIC_GCD_USERNAME` / `COMIC_GCD_PASSWORD` / `COMIC_GCD_ENABLED` work
as first-boot env bootstraps like the other provider keys (the DB value
wins once saved).

### Weekly refresh + staleness (`/admin/metadata` → Settings)

| Setting | Type | Default | Notes |
|---|---|---|---|
| `metadata.weekly_refresh_enabled` | bool | **false** | Off by design — auto-fetching burns provider quota. Live flip (no restart). |
| `metadata.weekly_refresh_cron` | string | `0 0 4 * * 0` | 6-field cron expression. Default = Sunday 04:00 UTC. **Cron-string changes need a server restart.** The enabled bool is live. |
| `metadata.weekly_refresh_window_days` | uint | 14 | Mylar pattern — series with a published issue inside this window get re-fetched every weekly run. Older series only re-fetch when stale. |
| `metadata.stale_after_days` | uint | 180 | A series is "stale" when `last_metadata_sync_at IS NULL` or older than this. Drives both the weekly cron's stale branch and `/libraries/{slug}/metadata/refresh?scope=stale`. |

## Operations

### Manual bulk refresh

When you've added new credentials and want to backfill matches:

```bash
# Replace {slug} with the library slug. scope can be:
#   unmatched — series with zero external_ids rows
#   stale     — never-synced or older than stale_after_days
#   all       — every active non-paused series in the library
#   recent    — series with an issue published inside the window
curl -X POST 'https://comics.example.com/api/libraries/{slug}/metadata/refresh?scope=unmatched' \
     -H "Cookie: __Host-comic_session=…; __Host-comic_csrf=…" \
     -H "X-CSRF-Token: …"
```

Response shape:

```json
{
  "library_id": "01234567-…",
  "scope": "unmatched",
  "series_eligible": 47,
  "jobs_enqueued": 45,
  "jobs_coalesced": 2,
  "jobs_failed": 0
}
```

Bounded to 200 series per call (`REFRESH_BATCH_CAP`). Re-trigger to
drain larger backlogs — the per-entity coalesce gate makes
repeated requests safe.

### Pause a series's auto-sync

Paused series are excluded from both the weekly cron and bulk
refresh fan-out. Useful for series where you've curated metadata by
hand and don't want provider data to even *appear* in the review
queue.

UI: series page → Actions → Pause auto-sync.
API: `POST /api/series/{slug}/metadata/pause`.

### Metron token auth, limits, and the budget bar

Verified against Metron's own docs on 2026-09-29
(`api/README.md` + `api/RATELIMIT.md` in
<https://github.com/Metron-Project/metron>, the
[March 2026 update](https://metron-project.github.io/blog/march-2026-update)
and the
[token-auth announcement](https://metron-project.github.io/blog/token-authentication)):

- **Auth.** `Authorization: Bearer <token>`, token generated under
  *API Tokens* on the account page. Basic auth still works but is on
  a phased deprecation path upstream. Folio sends the token when
  `metadata.metron.api_token` is set and falls back to Basic only when
  it isn't. The **Test** button exercises whichever one is configured.
- **Limits.** 20 requests/minute (burst) + 5,000 requests/day
  (sustained) per account; supporters get a higher sustained limit.
  Folio's local buckets sit at the base figures as a pre-flight; the
  *real* remaining budget comes from the `X-RateLimit-{Burst,Sustained}-
  {Limit,Remaining,Reset}` headers Metron returns on every response
  (`Reset` is a Unix timestamp), which Folio stores after each call.
- **Budget bar.** `/admin/metadata` → Providers shows, per provider,
  a bar for the headline window — Metron's daily budget as last
  reported upstream (or the local day bucket before the first call),
  ComicVine's local 200/h bucket (CV sends no budget headers) — plus
  the reset countdown and the last provider error. The Fetch-metadata
  dialog adds a one-line "Metron: 812 of 5,000 requests left today"
  note once a provider is under 20%.
- **429s** honour the upstream `Retry-After` (seconds or HTTP-date)
  instead of a fixed 60 s; 5xx and transport errors are retried up to
  3 times with jittered backoff (200 ms → 5 s) before the run parks.
- **Conditional requests.** Metron's *detail* endpoints send
  `Last-Modified` (some also `ETag`); Folio stores the validator with
  the cached payload and re-validates an expired row with
  `If-Modified-Since` / `If-None-Match`, so an unchanged series or
  issue costs a `304` (still one request against the budget) instead
  of a download. General list endpoints don't support conditional
  requests upstream, so searches are always full fetches.
- **Cover hashes are cached.** Candidate cover pHashes are kept for 30
  days per image URL (`metadata_cover_hash`), so re-running a search
  doesn't re-download the same covers.

### Grand Comics Database (GCD)

Verified against GCD's published OpenAPI schema (`/api/schema/`), the
GCD source (`apps/api/` + `settings.py` in
<https://github.com/GrandComicsDatabase/gcd-django>) and live API
responses on 2026-10-01:

- **Auth.** HTTP Basic with a comics.org account. The API is readable
  anonymously, but anonymous clients get **30 requests/hour**; an
  account gets **2,000/day**. Folio requires the account so it never
  runs on the anonymous tier. A wrong password returns 401 even on
  read-only routes, so the **Test** button checks it.
- **Limits.** Local pre-flight buckets: 100/hour and 2,000/day, plus a
  1 request/second floor. GCD sends no budget headers, so the
  Providers card's budget bar shows the local daily bucket. A 429
  honours GCD's `Retry-After`.
- **What a search costs.** A series search usually costs one request.
  The name-only route runs only when the exact-year route has no
  exact-name match, and a second results page is read only when the
  first has no exact match. An issue search for a series that isn't
  matched to GCD yet costs 1–2 requests, with no issue details fetched.
  Once a series is matched, issue searches read GCD's series
  **overview**: one request covers 50 issues and is cached for a day.
  Matching all 145 issues of Invincible costs about 4 requests (it used
  to cost about 309). Series and publisher lookups are cached for 7 days.
- **What an apply costs.** An issue apply costs one request, plus up to
  3 requests to collect variant covers (only while at least 50 of the
  hourly 100 requests remain, so variants never starve searches). A
  series apply costs 1–2 requests, and the follow-up "Detect from
  providers" split reuses the cached issue list.
- **What you get.** Per-story credits (writer, penciller, inker,
  colorist, letterer, editor, cover artist), characters and teams (with
  first appearances and deaths), genres, keywords as tags, the main
  story's title and synopsis, cover month and on-sale date, page count,
  price, age rating (GCD's free text is normalised, e.g. "Rated T+" →
  Teen, while Comics Code approval is left empty), barcode (UPC/EAN) and
  ISBN, volume, language, imprint (from the brand emblem when it names a
  distinct line such as Vertigo), series type and format (ongoing vs
  limited vs TPB/hardcover, which also helps the matcher tell a trade
  series from the ongoing one), and variant covers with their artist
  when the variant name gives it. GCD's credit data is free text:
  uncertain credits (`?`) and production staff are left out on purpose.
  The full field-by-field audit is in
  [`metadata-providers.md`](metadata-providers.md#field-audit-openapi-schema--folio).
- **Unstable schema.** GCD documents its API fields as subject to
  change. Folio reads them tolerantly, so a renamed or missing field
  leaves that value empty instead of failing the search. If GCD results
  suddenly come back sparse, check the Providers card's last error and
  file an issue with the response.
- **Series splits.** GCD splits long runs differently from ComicVine
  (e.g. Fantastic Four (1961) ends at #416 on GCD). **Analyze coverage**
  (series page → Details) finds which GCD series hold which of your
  issues and maps each run onto the right one. The series doesn't need a
  GCD match first: see [Analyze coverage](#analyze-coverage) below.
- **Covers aren't downloadable.** GCD serves every cover from
  `files1.comics.org`, which sits behind a Cloudflare bot challenge.
  The challenge refuses server-side downloads and browser hotlinks
  alike, and GCD's API has no image endpoint. Folio does not try to get
  around it. What you'll see:
  - GCD match candidates show a grey placeholder instead of a cover.
    The same fallback applies to any provider whose image fails to load.
  - Cover matching treats GCD candidates as cover-less, so ranking uses
    the text score (name, year, number, publisher, format).
  - **Apply cover** with a GCD match skips the cover cleanly. The apply
    records `cover_skipped_reason: "cover_unavailable: files1.comics.org
    refuses non-browser downloads (bot challenge)"` in its outcome and
    audit entry, and every other field still applies. To get a cover,
    apply it from ComicVine or Metron, or keep the archive's own cover.
  - Folio notices the challenge on the first refused request, then
    stops contacting the image host for an hour, so there are no
    retries and one log line per hour (`cover host answered with a bot
    challenge`). It checks again after that, so covers start working
    without a restart if GCD ever lifts the challenge.
- **License.** GCD data is CC BY-SA 4.0. Folio links every GCD-sourced
  series/issue back to comics.org in the Sources footer.

### Quota exhaustion

When a provider hits its minute, hour or day limit, the orchestrator
marks the run `awaiting_quota` + records `resume_after` from the
upstream `Retry-After` when one was sent. The dialog renders
"Providers are out of quota — try again shortly" instead of
"failed". The token bucket refills on the provider's own schedule
(CV: hourly window; Metron: minute + day windows). No operator
action needed.

If you're hitting quota constantly:
1. **Disable the lower-priority provider.** ComicVine has the
   tighter rate cap (200/hr) and richer dataset; Metron is faster
   (20/min × 60 = 1200/hr, 5,000/day) but has narrower coverage. If
   you don't need both, turn one off.
2. **Reduce weekly_refresh_window_days** so fewer series fall into
   the "recent" scope each weekly run.
3. **Bump stale_after_days higher** so the long-tail catch-up sweep
   touches fewer series.

### Reviewing low-confidence matches

There is no dedicated review-queue surface. A non-manual run (weekly
cron / bulk-refresh / scanner) only auto-applies an unambiguous
`SingleGood` strong match (and only when the library has
`metadata_auto_apply_strong_matches` on); MEDIUM (70-94) and LOW
(<70) candidates are recorded as `metadata_run_candidate` rows but
take no automatic action.

To resolve an ambiguous match, open the entity's **Fetch metadata**
dialog (series or issue page) — it lists the same candidates and lets
you pick + apply one. The **Runs** tab (`/admin/metadata`) drills into
the per-candidate detail for any past run if you want to see what a
given sweep found.

### Perceptual hash backfill

`POST /api/admin/metadata/phash-backfill` walks every
`issue_cover` row with NULL phash, decodes the on-disk bytes, and
writes the hashes. Bounded to 500 rows per call.

You only need this on existing libraries that pre-date the M9
phash extraction (cover hashes computed at write time for new
scans). Symptom: ranked candidate lists with no `cover_phash`
component in their score breakdown.

Audit-logged as `admin.metadata.phash_backfill`.

### Watching what's happening

- **`/admin/metadata` → Dashboard tab** — series total / matched /
  unmatched + applies-last-7-days. Per-provider quota gauges show
  remaining-hour and remaining-day token counts read straight from
  Redis.

- **`/admin/metadata` → Runs tab** — paginated `metadata_run` history.
  Each row drills into the per-candidate detail + the audit_log
  entries the apply emitted.

- **`/admin/activity` (filter chip = `metadata`)** — every metadata
  apply emits an audit_log row. Filter by `admin.metadata.*` to
  see who applied what to which series.

- **Sidebar Metadata badge** — live unmatched-series count. Hides
  at 0; click through to the dashboard.

## Pre-tagging libraries for free matches

The scanner recognizes external IDs in two on-disk forms — neither
counts against your provider quota because no search is required:

### MetronInfo.xml sidecar

If the archive carries a `MetronInfo.xml` file, every `<ID source="...">`
entry becomes an `external_ids` row on the issue with
`set_by='metroninfo'`. Sources Folio recognizes:
`comicvine`, `metron`, `gcd`, `marvel`, `locg`, `mal`, `anilist`,
`mangaupdates`, `isbn`, `upc`, `asin`, `doi`. Unknown sources are
silently dropped (no scanner crash).

Tools that write MetronInfo: metron-tagger, ComicTagger
(MetronInfo plugin), Mylar3 (recent versions).

### Series folder-name tags

Folder names like `Saga (2012) [cv-12345] [metron-67890] [gcd-99999]`
become `external_ids` rows on the *series* with
`set_by='scanner_folder_tag'`. Same source registry as MetronInfo;
prefixes are case-insensitive (`[CV-...]` works); unknown prefixes
are dropped.

Tools that write these: metron-tagger (default folder pattern), manual
tagging.

When you re-scan a folder whose tag changed, the writer's
user-precedence rule protects values you've pinned by hand — a
folder-tag refresh never overwrites a `set_by='user'` row.

## Troubleshooting

### "Invalid API Key" from ComicVine

Almost always a trailing newline on the pasted secret. Folio trims
whitespace before sending (since v0.3.x), so this should only
happen on first-paste before the trim landed. Re-paste and save.

### "No metadata providers configured + enabled" on Fetch metadata

The master toggle is off OR credentials are blank. The Providers
tab's "configured" indicator shows green when credentials are set,
yellow when set-but-disabled, gray when blank. Both green + enabled
is required.

### Search returns zero candidates for a series you know exists upstream

Three escape hatches live in the Fetch metadata dialog (WP-2.8); none
of them edit the series or issue row.

1. **Adjust query.** Expand *Adjust query* under the candidate list,
   change the name / start year / publisher (issue number on the
   issue dialog) and *Search with this query*. The overrides replace
   the local facts **for that run only** and are recorded on the run
   (`metadata_run.query.overrides`), so the Review queue and the
   dialog say "Searched as …". Series name normalization is
   aggressive (drops articles, common prefixes, year-suffixes), so
   typing the provider's exact title is usually enough. Setting a
   year also *pins* it: the orchestrator will not relax the year
   gate for a year you asserted.
2. **Paste provider URL.** Paste the ComicVine volume / issue page
   (`…/4050-<id>/` or `…/4000-<id>/`) or a Metron series / issue URL
   carrying the numeric id (`metron.cloud/series/<id>/` or the API
   form `metron.cloud/api/series/<id>/`) and click *Lookup*. The
   server fetches that exact record — through the same cache and
   rate bucket a search uses — and returns a completed run with it as
   the single HIGH candidate, so preview and apply work unchanged.
   Metron's public page links use slugs (`/series/saga-2012/`), which
   the API can't resolve; the dialog tells you to use the numeric id.
   URLs for providers that aren't configured + enabled are rejected
   with a 422 before any network call.
3. **Relaxed year gate (automatic).** The pre-filter drops any
   candidate whose start year is more than one year past the local
   year. When that gate empties the list — the classic "folder says
   2010, the real volume started 2015" case — the orchestrator
   re-scores the same provider results under the cover-aware gate
   (no extra provider call): a year-mismatched candidate survives
   only if its cover pHash confirms the match. The run is annotated
   (`query.year_gate_relaxed`) and the dialog shows *Year gate
   relaxed — candidates may be from a different volume*. The fallback
   needs a local cover hash (`issue_cover.phash`) and never fires
   when the year was supplied as an override.

If the provider's title differs significantly from yours (e.g.
yours says "The X-Men" and Metron has "Uncanny X-Men"), the
matcher's HIGH threshold (`metadata.auto_apply_threshold`, default 80)
won't fire — but the
candidate WILL appear in the dialog with a MEDIUM badge. Preview
+ apply still works.

### A series keeps getting wrong matches assigned

Add the correct external_id by hand via the `<ExternalIdsCard>` on
the series page; that pins the row as `set_by='user'` and prevents
future auto-matches from overwriting. If the series shouldn't be
touched by the weekly cron at all, turn its auto-sync off on the
series Details tab (it defaults off — it has to be opted in).

### Weekly cron is enabled but nothing is happening

Check `last_metadata_sync_at` on a few series. If all are recent,
the cron has nothing to do (the recent + stale scopes both find
zero eligible rows). The cron itself logs at INFO when it fires
(`metadata weekly refresh: starting sweep` + per-library
fan-out counts) — grep server logs for `metadata weekly refresh`.

Cron-string changes need a server restart; the enable toggle is
live. If you flipped the cron-string and the new schedule isn't
firing, restart the server.

### "provider rejected credentials" from Metron

- Token: re-check it under *API Tokens* on metron.cloud — revoked
  tokens 401 immediately. Paste again; Folio trims whitespace.
- Basic fallback: only used when the token field is empty. If you set
  a token, the username/password pair is ignored entirely, so a stale
  password can't be the cause.
- The Providers card shows the last error with its timestamp, cleared
  by the next successful call.

### Covers won't load in the MetadataMatchDialog

A GCD candidate always shows a grey placeholder: GCD's image host
refuses hotlinks (see [GCD](#grand-comics-database-gcd)), and that is
expected. For other providers it is likely a CSP issue. Folio's
`img-src` directive ships with an
allowlist of provider CDN hosts (CV's `comicvine.gamespot.com`,
Metron's `static.metron.cloud`, GCD's `files1.comics.org`). If a
candidate's `cover_image_url` is hosted somewhere else, the
browser blocks the image with a CSP violation. Check the browser
console for "blocked by Content Security Policy" entries and add
the host to `crates/server/src/middleware/security_headers.rs`.

### A field I want to apply is greyed out in the preview pane

It's `blocked_by_user` — the field has `set_by='user'` in
`field_provenance`. Admins can flip the **Override user-edited
fields** toggle at the top of the dialog to bypass the
precedence rule (audited as `metadata_apply_force`, or
`metadata_composite_apply_force` from the multi-provider merge view);
non-admins see the field as read-only, and the API rejects the flag
from them with `403 auth.permission_denied` before anything runs
(`api::extractors::AdminGatedOverride`).

## Disaster recovery

### "I want every series to re-fetch from scratch"

```sql
-- 1. Wipe external_ids for the library
DELETE FROM external_ids
WHERE entity_type = 'series'
  AND entity_id IN (
    SELECT id::text FROM series WHERE library_id = '<library_uuid>'
  );

-- 2. Reset last_metadata_sync_at so the next refresh treats them as fresh
UPDATE series
SET last_metadata_sync_at = NULL,
    metadata_sync_paused = false
WHERE library_id = '<library_uuid>';
```

Then `POST /libraries/{slug}/metadata/refresh?scope=unmatched` will
walk every series.

### "A bulk apply went wrong"

Every apply writes an `audit_log` row + flips
`metadata_run_candidate.applied_at`. To find the offending run:

```sql
SELECT actor_id, action, payload, created_at
FROM audit_log
WHERE action LIKE 'admin.metadata.%'
  AND created_at > NOW() - INTERVAL '1 hour'
ORDER BY created_at DESC;
```

There's no automatic rollback — apply writes are committed
transactionally. To revert: re-run the apply with the previous
provider's data, or manually edit the affected entity rows.

## Archive writeback

Per-library opt-in that inverts the apply pipeline: ComicInfo +
MetronInfo XML get rewritten **into the archive** on every apply
instead of just being committed to the DB. Downstream consumers (OPDS,
ComicTagger, Komga, Mylar3, KOReader Sync) see the same data Folio
sees. See [`metadata-sidecar-writeback.md`](metadata-sidecar-writeback.md)
for the architecture; this section is the operator playbook.

### Enabling on a library

Two flags on the `libraries` row, both in
`/admin/libraries/{slug}/settings`:

1. **`allow_archive_writeback`** — master kill-switch. Off = Folio is
   read-only against this library's archives. Default off.
2. **`metadata_writeback_enabled`** — routes metadata apply through
   the XML composer. Requires the master flag. Default off.

Migration recipe per library:

1. Flip both toggles on a low-stakes library first.
2. Pick a single series, click **Fetch metadata** → **Apply**.
3. Open one of the rewritten archives:
   `unzip -p path/to/issue.cbz ComicInfo.xml | head -50`
4. Confirm the XML carries the expected fields (per-role credits,
   `<Web>`, `<Notes>` Folio attribution line with the
   `[CVDB<id>]` token, the variant `<Pages>` / structured
   `<Credit>` elements in MetronInfo).
5. Watch the library's **Health** tab for any
   `MetadataDriftFromXml` row (see below).
6. Repeat on the rest of the libraries.

### Drift dashboard

When a writeback-enabled library has user pins that landed AFTER the
issue's last sidecar rewrite, the library's Health tab shows a
`MetadataDriftFromXml` row (severity `info`) with the count of drifted
issues. This means: the DB knows about the edit; the archive XML still
has the old value; downstream consumers reading the file see stale
data.

Click **Flush pins to archives** (or `POST /libraries/{slug}/metadata-drift/flush`)
to compose XML from current DB state and enqueue per-issue rewrite
jobs across every affected series. The row disappears once
`last_rewrite_at` ticks past the pin `set_at` on every drifted issue.

The synthesized row is admin-only and not persisted — it's recomputed
every time the Health endpoint is queried, so a successful flush
clears it on the next page refresh.

### Rollout progress metric

Prometheus gauge `folio_metadata_writeback_libraries_remaining`
counts libraries still in legacy DB-direct mode. Refreshed at server
boot and weekly at 04:00 UTC Monday. Once the gauge stays at zero
across all your libraries, the follow-up code-quality cleanup PR can
drop the legacy DB-direct apply branch — flag a maintainer.

### Troubleshooting writeback

- **Apply succeeded but archive bytes didn't change**: check
  `archive_backup_retain_count` on the library (defaults to 1). The
  rewrite rotates `.bak` slots; the original is preserved at
  `<filename>.cbz.bak` (older slots at `.cbz.bak.1`, `.cbz.bak.2`, …)
  until rotated out. The rewrite worker logs every successful swap
  with the source + tmp + final paths.
- **Backups piling up on disk**: the daily 04:45 UTC sweep deletes
  `.bak` files older than the library's `archive_backup_retain_days`
  (default 30). Set it to `0` to keep backups forever; lower it, or
  lower `archive_backup_retain_count`, to reclaim space sooner. The
  library health page's backup-storage card shows the current
  footprint.
- **`MetadataDriftFromXml` row appeared unexpectedly**: any user PATCH
  through the Edit sheet creates drift until the next apply. The
  Flush button is the operator-side resolution; the next provider
  apply would carry the pins forward anyway via the composer.
- **Rewrite stuck on a busy archive**: per-issue Redis mutex
  `archive:rewrite:<issue_id>` with 120s TTL. A crashed worker
  releases on TTL; a still-running rewrite holds the lock until it
  finishes. Subsequent applies for the same issue skip with
  `archive busy (mutex)` in `ApplyOutcome.sidecar_skip_reasons`.

### Coverage after series matches (`/admin/metadata` → Settings)

| Setting | Type | Default | Notes |
|---|---|---|---|
| `metadata.coverage_after_series_apply` | string | `manual_only` | `off` \| `manual_only` \| `all`. Which successful series matches queue a coverage check of the matched series (see [Coverage after a series match](#coverage-after-a-series-match)). `manual_only` = matches you apply in "Match this series…"; `all` adds bulk applies from a batch review and automatic matches. About 1–3 provider requests per match; one check per series however many matches a batch applies, and none queued while 50 are already waiting. Unknown values fall back to `manual_only`. Live. |
| `metadata.coverage_auto_accept` | bool | **false** | Let that check accept a High-confidence, conflict-free result itself (ids recorded as provider-set, ranges as automated). Off: results wait for your Accept. Live. |

## Analyze coverage

The **Analyze coverage** button on a series' Details tab (admins only)
works out which ComicVine, Metron and GCD series hold which of this
series' issues. It needs no prior match: a folder that mixes a 1998
volume with its legacy-numbered #500+ run, or two relaunches in equal
parts, is handled the same way.

- **How it decides.** For each provider Folio collects up to 8 candidate
  series (ids it already has, Metron's own ComicVine/GCD
  cross-reference, and a series search on the name and aliases with no
  year filter; your library's publisher blacklist applies), lists every
  candidate's issues *with cover dates*, and assigns each of your issues
  to the series that has that number **and** a cover date within six
  months of yours (or the same year ±1 when a month is missing). That is
  what tells Daredevil (1998) #1 from Daredevil (1964) #1. The series
  holding the most of your issues becomes the provider's main series;
  every other series becomes a range mapping for the issues it holds.
- **Runs in the background.** Three providers and ComicVine's one
  request per second don't fit a normal request, so the button queues a
  job and the card updates when it finishes (usually 10–60 s). The result
  is kept for 24 hours.
- **Reading the result.** On a wide screen a grid shows your issues
  (collapsed into runs like "#1–5") against each provider; on a phone the
  grid becomes a list per provider. Each provider shows its confidence
  and why, the request count, issues **no series of that provider has**,
  specials that sit in another series but can't be ranged, conflicts with
  your own settings, and stale automatic mappings (with a remove button —
  Folio never removes a mapping on its own).
- **Confidence.** *High* means the main series matches your series'
  name and start year exactly (or is the series you linked yourself)
  **and** every issue's cover date agrees. *Medium* means one of the two;
  *Low* neither. A local start year that differs from the provider's
  (a 1999-labelled folder for a 1998 volume) is the usual reason for
  Medium.
- **Accept / Choose series.** **Accept** saves the main series as your
  confirmed choice and writes the range mappings. **Choose series** picks
  another candidate as the main first. Ranges that overlap one of your
  own mappings are skipped and listed; existing mappings are never
  overwritten or deleted.
- **Accept high-confidence results** (checkbox next to the button)
  accepts every High-confidence provider that changes something and
  conflicts with nothing you set, as soon as the analysis finishes. Those
  ids are recorded as provider-set, not as your choice.
- **Your links win.** If you linked a provider series yourself, the
  analysis is built around it and Accept never replaces it; to change it,
  edit the External IDs card first.
- **Budget.** Per analysis Folio spends at most 40 ComicVine requests (of
  200/hour), 30 Metron (it waits out the 20/minute burst once) and 30 GCD
  (of 100/hour). Issue lists are cached for 24 hours, so a second
  analysis usually costs one search per provider. A provider that hits
  its limit shows **Rate limited**; the others still finish.
- **Annuals and specials** (`Annual 1`, `14AU`, `½`) are never range
  bounds; they're listed instead.

### Batches use coverage instead of searching

Once a series has its provider series (accepted coverage, an applied
match, or a range mapping), **Fetch metadata → All issues** (and *Only
missing or partial*, a grid selection or a saved-view batch) looks each
issue up directly: Folio reads the provider series' issue list (cached
24 hours), picks the issue with the same number and a cover date within
six months of yours, and fetches just that issue — no search. The
fetched issue is still scored like any search result (cover comparison
included), and it appears in the match dialog as "Matched by series
coverage (number + cover date)". An issue the list doesn't have, whose
cover date disagrees, or whose cover doesn't match falls back to a
normal search.

The Review tab's batch header shows, per provider, how many issues were
answered directly and how many searched, with the reasons (for example
"GCD: 0 direct · 173 searched (173 no provider series)" means GCD has no
series linked — run **Analyze coverage** and accept it first to save
those searches). The issue fetched for the lookup is the one the apply
uses, so applying costs nothing extra for 24 hours.

**Repairing descriptions a series apply overwrote.** *Only missing or
partial* also picks issues whose description is the series description
(shared by two or more issues), even when they otherwise look complete;
descriptions you set by hand are left alone. Run it on such a series,
then **Fill missing** in the Review tab.

The older per-provider split detector
(`POST …/provider-ranges/detect`) is still available through the API.
It no longer runs after a series match; the coverage check below does.

### Coverage hints in "Match this series…"

Each series candidate in the match dialog shows how well it covers your
issues, by issue number and cover date — for example "Covers 160 of
your 173 issues · #600–611 aren't in this series". The hint is for your
information only: it never changes a candidate's score, confidence or
position in the list.

- **What it costs.** The top three candidates are checked when the
  search finishes; any other candidate only when you click **Check
  coverage**. Each check reads that provider series' issue list
  (ComicVine / Metron: one request per 100 issues; GCD: its index plus up
  to four overview pages), which is then cached for 24 hours — the same
  cache the coverage check after a match and batch lookups use, so the
  series you pick costs nothing extra later. Per series and provider the
  hints spend at most the coverage budget per hour (ComicVine 40, Metron
  30, GCD 30 requests).
- **"Coverage not computed".** Shown with the reason when a check was
  skipped: the hour's budget for that series is spent, the provider is
  rate limited, slow (a check stops after 30 seconds), not configured, or
  can't list a series' issues.

### Coverage after a series match

When you apply a series match (setting `manual_only`, the default; `all`
also covers bulk and automatic matches), Folio checks the matched
provider series against your issues in the background — only for the
providers you matched, using the matched series as that provider's main
series. If some issues belong to another series of that provider (a
legacy-renumbered relaunch, say), it finds that series with one issue
search and proposes a range mapping. Usually 1–3 requests per match.

The result appears at the top of the **Coverage analysis** section on
the series' Details tab: "This folder spans 2 Metron series — accept
the range?" with **Accept**, or "your match covers all 173 issues —
nothing left to accept". Nothing is written until you accept, unless
`metadata.coverage_auto_accept` is on and the result is High confidence
with no conflict with what you set. Your own links and mappings are never
replaced. Set `metadata.coverage_after_series_apply` to `off` to stop it.

### Refresh this series (guided)

**Fetch metadata → Refresh this series…** in a series' ⋯ menu (admins)
does the whole refresh in one dialog, step by step:

1. **Series match.** If the series is already matched you see its
   provider ids — **Keep current match** skips straight on without
   searching. Otherwise (or with **Search for a match**) the usual match
   list appears, coverage hints included.
2. **Coverage.** After a match you applied here, the dialog waits for the
   coverage check that match queued (usually 1–3 requests). Without one it
   offers **Analyze coverage** (at most 40 ComicVine, 30 Metron and 30
   GCD requests). Accept or skip each provider; nothing is saved until
   you accept.
3. **Per-issue fetch.** Choose *Only missing or partial* or *All issues*.
   The dialog shows, per provider, how many issues will be looked up
   directly through the series' coverage (one request each, no search)
   and how many will be searched (one or two requests each).
4. **Review.** When the fetch finishes: the strong / need-review /
   no-match counts, how many issues each provider answered directly,
   **Accept all strong** and **Fill missing**, and **Open in Review** for
   issue-by-issue work (and *Replace all*).

You can close the dialog at any time: reopening it within 24 hours picks
up where it got to (a fetch still running, or its results). Every step is
also still available on its own in the same menu and on the Details tab.

## Files referenced

- [`docs/dev/metadata-providers.md`](metadata-providers.md) — developer architecture
- [`docs/dev/metadata-sidecar-writeback.md`](metadata-sidecar-writeback.md) — writeback architecture + risk matrix
- [`docs/dev/schema-restructure.md`](schema-restructure.md) — M0 schema changes
- [`docs/dev/runtime-configuration.md`](runtime-configuration.md) — env-vs-DB settings split (general)
