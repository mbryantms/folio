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
   - **Metron**: free; create an account at <https://metron.cloud/>
     and use your username + password (HTTP Basic). Rate limit:
     30 requests/minute, 5000 requests/day.

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
| `metadata.metron.username` | string | — | HTTP Basic username. |
| `metadata.metron.password` | secret | — | AEAD-sealed at rest. |
| `metadata.metron.enabled` | bool | false | Master toggle. |

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

### Quota exhaustion

When a provider hits its hour or day limit, the orchestrator marks
the run `awaiting_quota` + records `resume_after`. The dialog renders
"Providers are out of quota — try again shortly" instead of
"failed". The token bucket refills on the provider's own schedule
(CV: hourly window; Metron: minute + day windows). No operator
action needed.

If you're hitting quota constantly:
1. **Disable the lower-priority provider.** ComicVine has the
   tighter rate cap (200/hr) and richer dataset; Metron is faster
   (30/min × 60 = 1800/hr) but has narrower coverage. If you don't
   need both, turn one off.
2. **Reduce weekly_refresh_window_days** so fewer series fall into
   the "recent" scope each weekly run.
3. **Bump stale_after_days higher** so the long-tail catch-up sweep
   touches fewer series.

### Reviewing low-confidence matches

`/admin/metadata` → **Review queue** tab. Shows every
`metadata_run_candidate` row in the MEDIUM (70-94) or LOW (<70)
buckets that hasn't been applied or dismissed.

For each row you can:
- **Review** → opens the standard MetadataMatchDialog for the
  entity, lets you pick + apply
- **Dismiss** → marks the candidate as ignored; it stops showing up
  in the queue

Dismissed candidates stay in the DB (under `metadata_run_candidate.dismissed_at`)
so you can audit who-dismissed-what later.

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
  unmatched + review-queue depth + applies-last-7-days. Per-provider
  quota gauges show remaining-hour and remaining-day token counts
  read straight from Redis.

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

Series name normalization is aggressive (drops articles, common
prefixes, year-suffixes). Try editing the series name on the
series page to match the provider's exact title, then re-search.

If the provider's title differs significantly from yours (e.g.
yours says "The X-Men" and Metron has "Uncanny X-Men"), the
matcher's HIGH threshold (default 95) won't fire — but the
candidate WILL appear in the dialog with a MEDIUM badge. Preview
+ apply still works.

### A series keeps getting wrong matches assigned

Inspect the review queue for that series. If a low-confidence
match keeps re-surfacing, dismiss it explicitly — the dismiss
flag survives across runs. Alternatively, add the correct
external_id by hand via the `<ExternalIdsCard>` on the series
page; that pins the row as `set_by='user'` and prevents future
auto-matches from overwriting.

### Weekly cron is enabled but nothing is happening

Check `last_metadata_sync_at` on a few series. If all are recent,
the cron has nothing to do (the recent + stale scopes both find
zero eligible rows). The cron itself logs at INFO when it fires
(`metadata weekly refresh: starting sweep` + per-library
fan-out counts) — grep server logs for `metadata weekly refresh`.

Cron-string changes need a server restart; the enable toggle is
live. If you flipped the cron-string and the new schedule isn't
firing, restart the server.

### Covers won't load in the MetadataMatchDialog

Likely a CSP issue — Folio's `img-src` directive ships with an
allowlist of provider CDN hosts (CV's `comicvine.gamespot.com` +
Metron's `static.metron.cloud`). If a candidate's `cover_image_url`
is hosted somewhere else (e.g. a future GCD integration), the
browser blocks the image with a CSP violation. Check the browser
console for "blocked by Content Security Policy" entries and add
the host to `crates/server/src/middleware/security_headers.rs`.

### A field I want to apply is greyed out in the preview pane

It's `blocked_by_user` — the field has `set_by='user'` in
`field_provenance`. Admins can flip the **Override user-edited
fields** toggle at the top of the dialog to bypass the
precedence rule (audited as `metadata_apply_force`); non-admins
see the field as read-only.

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

## Files referenced

- [`docs/dev/metadata-providers.md`](metadata-providers.md) — developer architecture
- [`docs/dev/schema-restructure.md`](schema-restructure.md) — M0 schema changes
- [`docs/dev/runtime-configuration.md`](runtime-configuration.md) — env-vs-DB settings split (general)
