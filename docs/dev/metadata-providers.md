# Metadata providers

The metadata-providers subsystem fetches series + issue metadata from
external sources (ComicVine, Metron, and — since WP-6.1 — the Grand
Comics Database), ranks candidates
against your local entities, and applies the chosen match back to the
DB with full provenance tracking.

This document is the developer-facing architecture reference. For
operator-side tuning (API keys, weekly refresh, troubleshooting),
see [`metadata-operator-guide.md`](metadata-operator-guide.md). For
the M0 schema changes that made this possible, see
[`schema-restructure.md`](schema-restructure.md). For the follow-up
plan that inverts the canonical-source-of-truth from DB to archive
XML (per-library opt-in flag, drift surfacing, flush button), see
[`metadata-sidecar-writeback.md`](metadata-sidecar-writeback.md).

## Layering

```
┌────────────────────────────────────────────────────────────────┐
│ HTTP surface                                                   │
│   /series/{slug}/metadata/{search,candidates,apply,…}          │
│   /admin/metadata/{dashboard,runs,auto-synced,phash-backfill}  │
└──────────────────────┬─────────────────────────────────────────┘
                       ▼
┌────────────────────────────────────────────────────────────────┐
│ jobs/metadata_search + jobs/metadata_apply (apalis workers)    │
│  - per-entity Redis coalesce gate (SET NX EX)                  │
│  - dispatch into orchestrator + apply                          │
└──────────────────────┬─────────────────────────────────────────┘
                       ▼
┌────────────────────────────────────────────────────────────────┐
│ metadata/orchestrator                                          │
│  - run lifecycle (metadata_run, metadata_run_candidate)        │
│  - fan-out per enabled+configured provider                     │
│  - matcher::score_*_with_phash → rank → persist                │
└──────┬─────────────────────────────────────────┬───────────────┘
       ▼                                         ▼
┌──────────────────┐   ┌──────────────────────────────────────────┐
│ metadata/        │   │ metadata/cache + metadata/rate_limit     │
│  provider impls  │   │  - TTL-bounded GenericMetadata cache     │
│  (comicvine.rs,  │   │  - Redis token bucket per provider       │
│   metron.rs)     │   │  - velocity caps (CV 1/sec; Metron 30/m) │
└──────────────────┘   └──────────────────────────────────────────┘
                       ▼
┌────────────────────────────────────────────────────────────────┐
│ metadata/apply + metadata/diff                                 │
│  - apply: writes scalar columns + junctions + external_ids +   │
│    field_provenance + cover (apply_cover); per-entity audit    │
│  - diff: same fetch path, no writes — drives M5 preview pane   │
└──────────────────────┬─────────────────────────────────────────┘
                       ▼
┌────────────────────────────────────────────────────────────────┐
│ metadata/writers                                               │
│  - single audited DB write surface                             │
│  - upsert_person/character/team/…/publisher/imprint/universe   │
│  - set_external_id (user-precedence rule)                      │
│  - apply_cover (writes issue_cover row + phash)                │
└────────────────────────────────────────────────────────────────┘
```

## Provider abstraction

Every concrete provider implements the [`MetadataProvider`][provider]
trait. The trait's only shape Apply jobs see is `GenericMetadata` —
the CV-or-Metron dialect dies at the client boundary. Adding a new
source means writing one client struct + one trait impl + adding the
prefix to [`Source::from_str`][source-fromstr] in
`crates/server/src/metadata/identifier.rs`.

Providers don't compete; they stack. The orchestrator fans out
sequentially in priority order (currently Metron → ComicVine → GCD,
hard-coded in [`build_providers`][build-providers]) and merges
ranked candidates across all of them. A single search may return
Metron's `Saga (2012, Image)` *and* CV's `Saga (2012, Image)` as
separate candidates — the user picks which provenance to trust.

## Rate limiting

Two layers stacked, with different jobs:

```
user click → [METADATA_FETCH governor: per-IP] → enqueue job
                                                         ↓
                                                apalis worker
                                                         ↓
                                  [Redis token bucket: per-provider]
                                                         ↓
                                                outbound HTTP call
```

- **Per-IP governor** — `tower_governor`, 30 req/min/IP, gates the
  user-triggered API endpoints. A single misbehaving client can't
  fill the job queue.
- **Per-provider Redis token bucket** — Lua-script atomic decrement +
  TTL refresh. Keys: `metadata:bucket:comicvine:hour`,
  `metadata:bucket:metron:min` (20), `metadata:bucket:metron:day`
  (5,000), `metadata:bucket:gcd:hour` (100), `metadata:bucket:gcd:day`
  (2,000). Survives restarts (the bucket state lives in Redis, not
  in-process); shared across replicas.

Workers reserve N tokens before each HTTP call. Token-bucket deny
requeues the job with `backoff = quota_resets_at - now + jitter`.

### HTTP resilience (WP-2.9)

Both clients send through
[`metadata::http::send_with_retry`](../../crates/server/src/metadata/http.rs):

| Concern | Behaviour |
|---|---|
| Client build | `build_client`: `connect_timeout(10s)`, total `timeout(30s)`, `redirect(Policy::limited(2))`, provider user-agent. |
| Retry | Transport errors + 5xx retried up to 3× (4 attempts) with jittered exponential backoff — 200 ms base, 5 s cap, ×0.5–1.0 jitter. **4xx is never retried.** A retry whose backoff would end after the per-call deadline (75 s) is skipped. |
| Body cap | 8 MiB, enforced on `Content-Length` *and* on streamed bytes → `InvalidResponse`, not retried. |
| `Retry-After` | Parsed on 429 (delta-seconds or HTTP-date) into `QuotaExceeded { retry_after_secs }`; fallback 60 s (CV `status_code=107`: 3600 s). |
| Budget | Metron's `X-RateLimit-{Burst,Sustained}-{Limit,Remaining,Reset}` → [`metadata::budget`](../../crates/server/src/metadata/budget.rs), stored in Redis `metadata:budget:<provider>` after every response; `metadata:last_error:<provider>` records the last failure (cleared on success). ComicVine's budget is derived from the local hourly bucket. |
| Auth (Metron) | `MetronAuth::from_config`: `Authorization: Bearer <token>` when `metadata.metron.api_token` is set, else Basic. |
| Auth (GCD) | `GcdCredentials::from_config`: HTTP Basic from `metadata.gcd.username` + `metadata.gcd.password` (both required — the anonymous tier is 30 req/h). |
| Budget (GCD) | No budget headers upstream; the bar is the local `gcd:day` bucket (2,000/day = the upstream user throttle). 429 carries DRF's `Retry-After`. |

The bucket reservation happens **once**, before the retry loop —
retried requests count against the same upstream window either way.

### Cache tables

| Table | Key | Payload | TTL |
|---|---|---|---|
| `metadata_cache` | `(provider, entity, external_id)` | normalized `GenericMetadata` JSON + `schema_version` (`cache::CACHE_SCHEMA_VERSION`, 2 since WP-7.8; a row of another version is a miss) + `etag` / `last_modified` validators | 24 h issue / 168 h series (settings) |
| `metadata_cover_hash` | provider image `url` | `phash` / `dhash` / `ahash` | 30 days |

`cache::get_or_revalidate` is the single-flight read path for detail
fetches: an expired row with validators is sent as
`If-None-Match` / `If-Modified-Since` via
`MetadataProvider::fetch_*_conditional`; a `304` refreshes `fetched_at`
and serves the stored body. Providers without conditional support
(ComicVine) use the trait default, which never yields a 304. Metron
supports `Last-Modified` on *detail* endpoints only — list endpoints
are always full fetches upstream.

`phash::fetch_and_hash_cover` reads `metadata_cover_hash` first and
fetches through one pooled `ssrf::shared_public_client` (DNS answers
vetted by the client's resolver, redirect hops by its policy) — the
per-fetch client + re-download per search are gone.
When *every* enabled provider is quota-exhausted, the orchestrator
marks the run `awaiting_quota` + sets `resume_after`; the dialog
UI renders a "providers are out of quota" state instead of "failed".

Worker concurrency is intentionally bounded to 1 per job type — the
per-provider velocity cap already serializes through a
per-instance mutex (the CV client's 1-req/sec rule) and running
multiple search workers concurrently gains nothing on the happy
path while risking burst-deny.

## Run lifecycle

Every search creates a `metadata_run` row with status `queued`. The
worker flips it to `searching` on pickup. Each per-provider call's
ranked results land as `metadata_run_candidate` rows (ordinal 0 =
best match). When all providers finish, status → `completed` and
`finished_at` stamps. Errors → `failed` + `error_summary`. Quota
exhaustion → `awaiting_quota` + `resume_after`.

The candidate rows survive the run, so the UI can re-render the
ranked list without re-fetching. Per-entity Redis coalesce keys
(`metadata:search:series:{id}` / `metadata:search:issue:{id}`,
`SET NX EX 60s`) collapse rapid re-clicks while one run is in
flight.

`metadata_run.query` starts as the serialized `StoredQuery` (the
*effective* facts the run searched with) and gains small notes via
`orchestrator::annotate_query` (WP-2.8): `overrides` (which fields
the user replaced for that run), `year_gate_relaxed` (the hard year
gate emptied the list and the cover-aware re-score ran), and `lookup`
(`{source, external_id, url?}` — the run came from
`POST …/metadata/lookup`, which fetches one provider record through
`apply::fetch_*_detail` and persists it via
`orchestrator::finalize_lookup_run` as a single HIGH candidate with
`score_breakdown.lookup = true`, deliberately without a
`metadata_match_outcome` row). `GET …/metadata/candidates` surfaces
all of it as `query: SearchQueryView`.

## Matching engine

The matcher's architecture inverted in `matching-accuracy-1.0` M4 —
cover-pHash is now the **primary** bucket discriminant, not a small
bonus on top of text scoring. See
[`docs/dev/matching-accuracy.md`](matching-accuracy.md) for the
full pipeline + operator-knob inventory; the short version lives
here.

`matcher::score_*_with_phash` produces a `Score` with text-only
components + the raw cover Hamming distance:

| Component       | Weight | Sources |
|-----------------|--------|---------|
| name            | 45     | sanitize_title + Ratcliff/Obershelp similarity (M2) |
| year            | 20     | exact match=1, off-by-one=0.75, NULL=0.5 |
| publisher       | 15     | sanitize_title equality + substring credit; NULL=0.5 |
| issue_number    | 15     | issue queries only; series queries collapse to 0 |
| volume          | 5      | reserved (providers don't return this in search) |
| cover_hamming   | —      | M4: raw bit-distance, NOT folded into `total` |

`Score::bucket()` consults `cover_hamming` FIRST. The ComicTagger
ladder (lifted verbatim):

| Cover Hamming                       | Bucket                                                       |
|-------------------------------------|--------------------------------------------------------------|
| 0–8 (`STRONG_SCORE_THRESH`)         | HIGH — cover decides regardless of text                      |
| 9–16 (`MIN_SCORE_THRESH`)           | MEDIUM (primary cover)                                       |
| 9–12 (`MIN_ALTERNATE_SCORE_THRESH`) | MEDIUM — tighter when winning cover came from an alternate   |
| 17+                                 | LOW (cover veto sinks even a perfect text match)             |
| `None` (no phash on either side)    | text fallback at operator thresholds                         |

Text-fallback thresholds are operator-tunable:

- `metadata.auto_apply_threshold` — HIGH cutoff. Default 80 (was
  hardcoded 95 pre-M1; unreachable for series scoring with text
  ceiling of 90).
- `metadata.match_medium_threshold` — MEDIUM cutoff. Default 60.

**Variant covers (M5)**: `score_*_with_phash` takes
`candidate_cover_phashes: &[Option<i64>]` — slot 0 is the primary
cover, slots 1.. are alternates. The matcher picks the minimum
Hamming and flags `Score::matched_via_alternate=true` when the
winner came from a non-primary slot, which routes the bucketer
through the stricter alternate ceiling.

**Gap-to-next-best guard (M4)**:
`orchestrator::finalize_ranking` looks at the top two
cover-Hamming candidates after sort — if both are HIGH-eligible
(≤ 8) but within 4 bits of each other (`MIN_SCORE_DISTANCE`), the
winner downgrades to MEDIUM. Two near-identical covers in the same
candidate set means we can't be confident which is right — the
user picks explicitly.

**Pre-filter (M3)**: `orchestrator::pre_filter_series` drops
candidates BEFORE scoring on (a) hard year gate
(`cand > local + 1`) and (b) per-library
`metadata_publisher_blacklist`. Pre-M3 these scored Medium because
the year/publisher components gave partial credit; the gate now
removes them outright.

Local phash comes from `issue_cover` (preferring
`source_provider='archive_extracted'` over provider-applied rows so
user-pinned images win over potentially-wrong prior matches).
Candidate phashes are computed on-the-fly from
`SeriesCandidate.cover_image_url` + `alternate_cover_urls` via a
parallel fan-out — see `fetch_phashes_per_candidate` in
`orchestrator.rs`. Capped at
`metadata.alternate_cover_fetch_cap` URLs per candidate (default
3, settable to 0 to disable variant fetching).

**Cover page selection (M6)**: the scanner stamps
`issue.cover_page_index` from ComicInfo's
`<Page Type="FrontCover" Image="N"/>` marker when present;
defaults to 0 (page 0) otherwise. Both the post-scan thumbnail
worker and the phash pipeline read this column so multi-cover
archives surface the right image to the matcher instead of always
the first page.

## Apply pipeline

`apply::apply_series` / `apply::apply_issue` walk every field defined
in `MetadataField` and decide per-field whether to write. The
single source of truth for the decision is
[`apply::should_apply`](../../crates/server/src/metadata/apply.rs):

```rust
fn should_apply(db_has_value, provenance, field, args) -> bool {
    if args.selected_fields.as_ref().is_some_and(|s| !s.contains(field.key())) {
        return false;  // M5 preview pane opt-in
    }
    if provenance[field] == "user" && !args.override_user_edits {
        return false;  // user-precedence rule
    }
    if !db_has_value { return true; }      // empty cell → fill
    args.mode == ApplyMode::ReplaceAll      // present cell → mode decides
}
```

The same predicate is mirrored in `diff::classify_field` so the M5
preview pane's per-field rows compute the same decision as the
write path.

`should_apply == true` for a field → the apply layer routes to the
appropriate writer:

- Scalar fields → `apply_series_updates` / `apply_issue_updates`
  (single SQL UPDATE per entity, batched from a `SeriesUpdates` /
  `IssueUpdates` struct)
- Junctions (credits, characters, teams, …) → `writers::set_issue_*` /
  `set_series_*` helpers that maintain the junction table + the CSV
  read-cache columns on the parent
- External IDs → `writers::set_external_id`
- Covers → `writers::apply_cover` (writes bytes + issue_cover row +
  per-cover phash)

After every successful write, `write_provenance_for_applied` emits
one `field_provenance` row per applied field with
`set_by=SetBy::Provider(source)` + the provider's external id.

### Credit roles: one canonical form (WP-8.1)

Three role vocabularies meet at the credit junctions:

| Layer | Spelling | Example |
| --- | --- | --- |
| Provider mappers (`CreditCandidate.role`) | ComicInfo PascalCase via `provider::canonicalize_role`, else the raw role lowercased | `Writer`, `CoverArtist`, `journalist` |
| MetronInfo XML `<Role>` | the schema's `roleValues` enumeration | `Writer`, `Cover`, `Ink Assists` |
| **Storage** (`issue_credits.role`, `series_credits.role`), the per-role CSV rebuild, filters, saved views, `/creators`, the web UI | **lowercase snake_case** | `writer`, `cover_artist`, `ink_assists` |

[`writers::set_issue_credits`](../../crates/server/src/metadata/writers.rs)
is the single write surface for provider credits and folds every role
through
[`provider::canonical_credit_role`](../../crates/server/src/metadata/provider.rs):
each known spelling (`Writer`, `Script`, `penciler`, `Artist`, `Cover`,
`Cover Artist`, `CoverArtist`, `Editor In Chief`, `colourist`, …) maps to
one of the eight keys (`writer`, `penciller`, `inker`, `colorist`,
`letterer`, `cover_artist`, `editor`, `translator`); any other role is
kept, lowercased and snake_cased. The function is idempotent. The
scanner already writes the eight keys (`CreditRole::as_str`).
`set_issue_credits` also stores the creator's **name** in the junction's
`person` column (the scanner does the same; `/creators`, `/people`, the
saved-view credit filters and the `series_credits` rollup key on it) and
dedupes `(role, person)` pairs.

Before WP-8.1 the apply wrote `Writer` rows with the person UUID in
`person`: the lowercase CSV rebuild never matched them, so a
non-writeback apply left `issues.writer` & co. empty (writer filters and
search missed the issue), and the series rollup then minted a "ghost"
`person` named with that UUID. Migration
`m20270601_000001_canonical_credit_roles` canonicalizes existing rows in
both junctions (same table in SQL — keep the two in sync), restores
names, drops the rows that collide after normalization, rebuilds the
eight CSV columns of the affected issues only, and deletes unreferenced
ghost people. Its `down` is a documented no-op (the old spellings aren't
recoverable). Tests: `metadata_apply.rs::apply_issue_canonicalizes_pascal_case_roles_and_fills_the_csv_cache`,
`migration_canonical_credit_roles.rs`, `provider::tests::canonical_credit_role_folds_every_spelling_onto_the_storage_key`.

### Metron links: `associated`, `alt_names`, reprints (WP-7.8)

Before WP-7.8 the Metron mapper read series `associated` entries as
`{id, name}`. The wire shape is `[{"id": 123, "series": "Saga (2012)"}]`,
so every entry deserialized with `name = None`: the list was always
empty, and the code fed it into `aliases` (wrong meaning anyway:
associated entries are *other* series). Now:

- `associated` → `GenericMetadata.related_series: Vec<ProviderSeriesRef
  { source, id, label, name, year, url }>` (the label's trailing
  "(YYYY)" becomes `year`). Untyped and symmetric upstream (a Django
  self-M2M). The series apply records them as external relationship rows
  (`relationships::external::record_provider_links`, called from
  `write_series_scalar_fields`, so the DB-direct **and** the sidecar
  writeback paths both do it) and the suggestion engine turns links
  between local series into suggestions. See
  `docs/dev/series-relationships.md` ("Provider links and external
  targets").
- `aliases` come from Metron's real alias field, `alt_names: [str]`
  (part of the series detail payload, no extra request).
- Issue `reprints` (`[{"id": 456, "issue": "Saga (2012) #1"}]`) were
  already parsed into `ReprintCandidate` but `writers::set_issue_reprints`
  had no caller, so `issue_reprints` stayed empty. The issue apply now
  writes them (`MetadataField::Reprints`):
  - **Decision** (`apply::reprints_should_apply`): the `should_apply`
    matrix — a `user` pin on `reprints` is sacred unless
    `override_user_edits`, `fill_missing` keeps an existing set — but
    **not** gated by `selected_fields`: reprints have no preview-pane row
    (like external ids they're additive provider data), so a preview
    selection would otherwise always drop them.
  - **Resolution** (`writers::reprint_specs`): each reprinted issue is
    matched to a local issue through `external_ids` (its Metron id) or the
    **id bridge** (the provider's cached detail of that issue in
    `metadata_cache` lists its `cv_id` / `gcd_id`, and a local issue is
    matched under one of them). No network. The label is always kept, and
    the provider id goes to `issue_reprints.reprinted_source` /
    `reprinted_external_id` so a row whose issue isn't local yet resolves
    later (`writers::resolve_pending_reprints`: from the
    `set_external_id` hook when the issue gains that id, and before every
    relationship-suggestion run).
  - **Writeback libraries**: neither ComicInfo nor MetronInfo carries
    reprints, so they're metadata-only rows like variant covers — the
    sidecar apply decides them and hands them to the rewrite job in
    `PostRewriteWrites.reprints` (+ `reprints_source`); the job writes
    them only after the XML is in the archive. The scanner never touches
    `issue_reprints`.
  - **Composite applies** take reprints from the most-preferred included
    provider that has any (only Metron today) and union `related_series`
    by `(source, id)`, attributing provenance to the donor.
- Not ComicInfo / MetronInfo fields: no composer
  (`sidecar_compose.rs`), parser or scanner-ingest change — the CLAUDE.md
  "new metadata field" checklist doesn't apply.
- `CACHE_SCHEMA_VERSION` is **2**: payloads cached by the old mapping
  (empty `related_series`, `associated` as aliases) are misses and get
  re-fetched.
- After a successful series apply (and an issue apply that wrote
  reprints DB-direct) the apply queues a relationship-suggestion run for
  the library (`apply::queue_relationship_suggest`, deduped per library);
  the writeback path's scoped rescan queues it for issue applies.

GCD's REST API exposes no series bonds or reprint links (they exist only
in its database dump) and ComicVine has no volume-to-volume links, so
WP-7.8 reads links from Metron only. GCD bonds are deferred until the
API serves them; GCD notes are not text-mined.

## Diff / preview pane

The M5 preview pane fetches `GET /series/{slug}/metadata/proposed-diff?run_id=…&ordinal=…&mode=…&override_user_edits=…`.
This re-runs the same logic as `apply` up to (but not including) the
write step, returning `DiffResp { rows, external_id_conflicts,
external_ids_new, changes_count }`. Each row carries:

- `current_value` + `proposed_value` (string-formatted regardless of
  underlying type — the UI renders uniformly)
- `decision` (`would_fill` / `would_replace` / `no_change` /
  `blocked_by_user` / `skipped_fill_missing_has_value` / `no_incoming_value`)
- `current_set_by` + `current_set_at` (provenance from
  `field_provenance` — drives the "Currently set by Metron, 2
  days ago" tooltip)

External-IDs conflicts (user-pinned `external_ids` row disagrees with
the candidate's value) surface separately so the preview can render a
per-source "Keep mine / Use theirs" toggle. The user's choices come
back to apply as `selected_fields: Vec<String>` + `override_external_id_sources: Vec<String>`.

The diff endpoint shares the provider detail-fetch cache with apply
(`metadata_cache` table; TTL 24h for issues, 168h for series), so
opening the preview pane is cheap after the first time.

## Scanner integration

The scanner reads metadata from two on-disk sources at ingest time:

- **ComicInfo.xml** — the de facto standard, written by Mylar3 /
  ComicTagger / metron-tagger. Per-issue fields land directly on
  the issue row.
- **MetronInfo.xml** — newer schema with richer creator credits +
  multi-source IDs. MetronInfo wins on overlapping fields (`§4.4`).

M8 extended this with two cross-source ingest paths:

1. **Full MetronInfo ID propagation** — MetronInfo's `<ID source="...">`
   list (`{"metron": ..., "comicvine": ..., "gcd": ..., "marvel": ...,
   "locg": ...}`) becomes one `external_ids` row per source with
   `set_by='metroninfo'`. Pre-tagged libraries land already-matched
   for every source the tagger knew.

2. **Folder-name identifier tags** — `[cv-12345]`, `[metron-67890]`,
   `[gcd-…]` etc. in the series folder name become `external_ids`
   rows with `set_by='scanner_folder_tag'`. Source prefixes are
   resolved through `Source::from_str` so adding a new alias works
   without touching the parser. Mixed-case is tolerated; unknown
   prefixes are silently dropped.

Both paths protect user-pinned values via `set_external_id`'s
precedence rule — rescanning a folder whose tag changed never
overwrites a value the user pinned by hand.

## Weekly refresh + bulk dispatch

`metadata.weekly_refresh_enabled = false` by default. When operators
opt in, [`scheduler::register_metadata_weekly_refresh`](../../crates/server/src/jobs/scheduler.rs)
fires on the configured cron (default `0 0 4 * * 0` = Sunday 04:00 UTC),
walks every library, and runs two scope fan-outs per library:

1. **Recent** — series with a published issue inside `metadata.weekly_refresh_window_days`
   (Mylar pattern; default 14)
2. **Stale** — series where `last_metadata_sync_at` is null or older
   than `metadata.stale_after_days` (default 180)

Each scope is bounded by `REFRESH_BATCH_CAP = 200` per library per
fire; operators re-trigger via `POST /libraries/{slug}/metadata/refresh?scope=stale|unmatched|all|recent`
to drain larger backlogs. The per-entity coalesce gate dedupes
overlap between the two scopes automatically.

## Cover-image perceptual hashing

[`metadata/phash`](../../crates/server/src/metadata/phash.rs)
computes three complementary 64-bit hashes on every cover:

- **phash** (DCT-II) — the workhorse. Robust to JPEG re-encode + resize.
- **dhash** (gradient) — cheap. Catches contrast variations.
- **ahash** (average) — baseline cross-validator.

Hashes are written:
- At apply time → `writers::apply_cover` decodes the provider cover
  bytes + writes all three hashes alongside the row
- At scan time → the post-scan thumbnail job decodes the on-disk
  cover + upserts an `archive_extracted` `issue_cover` row with the
  hashes

`POST /admin/metadata/phash-backfill` walks NULL-phash rows + decodes
the local bytes + writes hashes. Bounded to 500 per call; operators
re-click for larger backlogs.

The orchestrator uses these for ranking — see "Matching engine"
above. Future use: a deduplication sweep that finds near-duplicate
issues by phash similarity.

## Grand Comics Database (WP-6.1)

[`metadata/gcd.rs`](../../crates/server/src/metadata/gcd.rs) speaks the
read-only Django REST API at `https://www.comics.org/api/`. It is the
third provider and the least like the others. Everything below was
checked against GCD's published OpenAPI schema
(`/api/schema/?format=json`, rendered at `/api/schema/redoc/`) and live
responses on 2026-10-01.

- **Endpoints.** Series search is `GET /api/series/name/{name}/[year/{year}/]`
  (`icontains`, sorted by name). The exact-year route runs first; the
  name-only route runs only when the year route has no exact-name hit.
  Each route reads a second page only while no exact-name hit has
  turned up (`SEARCH_PAGE_CAP = 2`), and exact names sort first before
  the result is truncated. Broad issue search is
  `GET /api/series/name/{name}/issue/{number}/[year/{key-date year}/]`,
  widened to the year-less route when the year route has no non-variant
  row. Narrowed issue search reads `/api/series/{id}/overview/`. Details
  are `/api/series/{id}/`, `/api/issue/{id}/`, `/api/publisher/{id}/`.
  Relations are hyperlinks; ids are parsed out of them.
- **Names with `/`.** GCD's Apache front end returns 404 for an encoded
  slash (`Batman%2FSuperman`), so a slashed title searches on its longest
  slash-free fragment (`search_name`). `icontains` still finds the
  series, and the matcher scores the full name.
- **Tolerant parsing.** GCD says its API fields may change, so the
  client reads `serde_json::Value` through alias lists (`str_field`,
  `int_field`, `entity_id`) instead of typed structs. A renamed field
  falls through to its alias (or `None`), a re-typed scalar is coerced,
  an unknown field is ignored, an item with no recoverable id is
  skipped, and a missing `results` envelope reads as "no results".
  Only a non-JSON body is `InvalidResponse`. The schema even
  disagrees with the wire: `longest_story` is declared a string but is
  a `Story` object. `tests/gcd_client.rs::renamed_and_unknown_fields_still_parse`
  pins this behaviour.
- **Free-text credits.** GCD stores credits per *story* as text
  (`"Stan Lee (signed as …); Sol Brodsky ? (see notes)"`). The client
  maps `comic story` script/pencils/inks/colors/letters/editing onto
  the ComicInfo roles, the `cover` story's pencils/inks onto
  `CoverArtist`, and issue-level `editing` onto `Editor` only when the
  annotation names an editor. Production staff such as
  `(publisher)` and `(art director)` are dropped. Placeholders (`None`,
  `[none]`, `?`, `typeset`, `various`, `anonymous`) and uncertain
  credits (a trailing `?` or a `(?)` group) are left out. `[as Pen Name]`
  and `(signed as …)` keep the real name. Characters parse
  `Team [Member [Alter ego]; …]; Character (first appearance)`. A
  bracket group with several members makes its head a **team**.
  `(first appearance)`, `(first full appearance)` and `(introduction)`
  set the first-appearance flag. A plain `(death)` note sets
  died-in-issue; `(death in flashforward)` does not.
- **Request economy.** There are three Redis caches. The series summary
  and publisher name are kept for 7 days. The **issue index**
  (`id`, `number` and `descriptor` per active issue) is kept for 24 h.
  Search results fill it at no cost, because series search returns full
  `Series` objects. **Overview pages** are also kept for 24 h. A narrowed
  issue search finds the issue's position among the series' distinct
  numbers in the index. That position gives the overview page
  (`position / 50 + 1`), which is read first, then its neighbours, up
  to 3 pages. The candidate (cover URL, dates, main-story title) comes
  from that row, with no issue-detail hydration. Issue details are
  hydrated (at most 2) only if the overview can't place the issue.
  Broad issue search no longer hydrates at all: the `IssueOnly` row
  carries the series id, descriptor and publication date.
- **Variants.** GCD models each variant as its own issue whose
  `variant_of` points at the base. Searches skip variant rows. An issue
  apply looks up same-number siblings in the cached index and fetches
  up to 3 sibling details (`VARIANT_DETAIL_CAP`). A sibling whose
  `variant_of` points back becomes a `VariantCoverCandidate`, with its
  label from `variant_name` and the artist parsed from it when
  unambiguous (`"… Color Cover - Cory Walker"`, `"Cover B by X"`,
  `"Chris Giarrusso Cover"`). Covers are nice to have and can't be
  downloaded anyway, so variant collection runs only while at least 50
  of the hourly 100 requests remain (`VARIANT_BUDGET_FLOOR`).
- **Splitter.** `list_series_issue_numbers` reads the series index
  (variants deduped). That costs one request, or none right after a
  search or series apply; the paginated overview would cost
  `⌈n/50⌉`. GCD splits Fantastic Four (1961) at #416, so auto-split
  maps the legacy #500+ run onto its own GCD series.
- **Covers are unreachable.** Every image URL is on
  `files1.comics.org`, which sits behind a Cloudflare managed challenge
  (`403` + `cf-mitigated: challenge`). The challenge applies to
  server-side fetches and to browser hotlinks, and the schema offers no
  image endpoint. Folio does **not** try to pass the challenge.
  `util::ssrf` classifies the response as `FetchBytesError::Challenged`,
  and [`metadata/cover_block.rs`](../../crates/server/src/metadata/cover_block.rs)
  then marks the host blocked for an hour, logging one `info` line.
  Until the memo expires:
  - cover hashing (`phash::fetch_and_hash_cover`) returns `None` without
    a request, so GCD candidates are cover-less and the matcher falls
    back to text scoring;
  - `writers::fetch_cover_bytes` returns `CoverFetchError::Blocked`,
    mapped to `ProviderError::CoverUnavailable`, which is not retryable.
    "Apply cover" records
    `cover_skipped_reason = "cover_unavailable: …"` and the rest of the
    apply proceeds;
  - variant rows keep their `source_url` with no local bytes.

  `files1.comics.org` stays in the CSP `img-src` allowlist, so hotlinks
  work again if the challenge is lifted. Until then, the web falls back
  to the grey placeholder when a provider image fails
  to load (`<ProviderCoverImage>`, used by the candidate card, compare
  view, cover gallery, cover viewer and entity pages). This applies to
  every provider. The URLs stay in the data, so the memo picks the host
  up again by itself if the challenge is ever lifted.
- **License.** CC BY-SA 4.0. The canonical comics.org links drive the
  attribution footer. The composer's `Notes` audit line stays
  CV/Metron-only, because its wording is the CC-BY-NC-SA one.

### Request cost (before → after this pass)

| Operation | Before | After |
|---|---|---|
| Series search, local year matches GCD | 2 (year + name routes) | 1 (year route has the exact name) |
| Broad issue search (no GCD series yet) | 3–4 (1–2 routes + 2 detail hydrations) | 1–2 (routes only) |
| Narrowed issue search, one issue | 2–5 (series detail + up to 4 details; +1 publisher cold) | 1–3 cold (series detail, unless a search cached the index, + 1 overview page; +1 publisher cold), **0 warm** |
| Match every issue of Invincible (2003), 145 issues | 309 | 4 (1 series + 3 overview pages) |
| Match every issue of Fantastic Four (1961), 416 issues | 1,335 | 10 (1 series + 9 overview pages) |
| Series apply + auto-split | 2–3 (series + publisher + series again for the splitter) | 1–2 (the splitter reads the cached index) |
| Issue apply | 1 (+0–2 summary cold) | 1 (+0–2 summary cold) + up to 3 variant siblings while ≥ 50 of the hourly budget remain |

All requests still go through the `gcd:hour` (100) and `gcd:day` (2,000)
buckets and the 1 req/s floor.

### Field audit (OpenAPI schema → Folio)

Every path and schema field GCD publishes. "Used" names the Folio
target. Fields that reach `GenericMetadata` flow through
`metadata/writers.rs` (user-precedence rule) on DB-direct libraries, and
through the sidecar composer on writeback libraries.

**Paths**

| Path | Status |
|---|---|
| `/api/series/name/{name}/year/{year}/` | used → series search, first route |
| `/api/series/name/{name}/` | used → series search widening (≤ 2 pages) |
| `/api/series/name/{name}/issue/{number}/year/{year}/` | used → broad issue search, first route |
| `/api/series/name/{name}/issue/{number}/` | used → broad issue search widening (≤ 2 pages) |
| `/api/series/{id}/` | used → `fetch_series`, series summary, issue index |
| `/api/series/{series_id}/overview/` | used → narrowed issue search (24 h page cache) |
| `/api/issue/{id}/` | used → `fetch_issue` (apply/preview), variant siblings, overview-fallback hydration |
| `/api/publisher/{id}/` | used → publisher name (7-day cache) |
| `/api/series/` | deliberately unused: a whole-catalog crawl (100k+ series, thousands of pages, past the daily budget). The name routes cover discovery |
| `/api/publisher/` | deliberately unused: a whole-catalog crawl. Folio only needs publisher names by id |
| `/api/issue/on_sale_weekly/{year}/week/{week}/` | deliberately unused: a global feed (2024 week 10 = 365 issues on 8 pages) of slim `IssueOnly` rows with no number, title or date. No Folio feature consumes upstream new releases: there is no pull list, and the `recent` refresh scope is driven by local files. Narrowing it to matched series would still mean reading every page |
| Pagination envelope `count` / `next` / `previous` | `next` used → page walking. `count` and `previous` deliberately unused, because the walk stops on `next = null` or the cap |

**`Issue`** (`/api/issue/{id}/`)

| Field | Status |
|---|---|
| `api_url` | used → GCD issue id (`external_ids`, canonical comics.org URL) |
| `series_name` | used → `series_name` + `year_began` (`"X (1961 series)"` split) |
| `descriptor` | used → issue-number fallback; variant label fallback |
| `number` | used → `issue_number` |
| `volume` | used → `volume` (positive integers only) |
| `variant_name` | used → `VariantCoverCandidate.label` + parsed `artist_name` |
| `title` | used → `title` (fallback: main story's title) |
| `publication_date` | used → `cover_date` (English month names, plus a day if printed). Year-only and non-English values fall back to `key_date` |
| `key_date` | used → `cover_date` fallback (`YYYY-MM-00` → 1st). The lenient form gives candidates their year |
| `price` | used → `price` (first listed price; comma decimals). The currency has no slot; non-decimal prices (`9d`) are dropped |
| `page_count` | used → `page_count` (`"36.000"` → 36; 0 dropped) |
| `editing` | used → `Editor` credits, plain or editor-annotated names only |
| `indicia_publisher` | used → `publisher` fallback when the series publisher can't be resolved |
| `brand_emblem` | used → `imprint` when it is a distinct line (`Vertigo` under DC). The publisher's own emblem, ≤ 3-character codes and emblems with digits are dropped |
| `isbn` | used → `isbn` identifier (first valid ISBN-10/13, digits only) |
| `barcode` | used → `upc` identifier (UPC-A ± add-on). EAN-13 becomes `gtin`, or `isbn` for a Bookland EAN when no ISBN is recorded |
| `rating` | used → `age_rating`, normalised to the ComicInfo vocabulary (`Rated T+` → Teen, `Parental Advisory` → Teen, `Mature Readers` → Mature 17+, `Explicit` → Adults Only 18+, `Ages 12+` → Everyone 10+, `All Ages` → Everyone). Comics Code text and unknown values → none |
| `on_sale_date` | used → `store_date`, full dates only. A partial `YYYY-MM` would invent a day |
| `indicia_frequency` | deliberately unused: neither ComicInfo nor MetronInfo has a frequency field, and it has no matching value |
| `notes` | used → `notes` |
| `variant_of` | used → variant handling (skipped in searches, grouped into the base's `variants`) |
| `series` | used → `series_external_id` (auto-split, summary lookup) |
| `indicia_printer` | deliberately unused: no Folio, ComicInfo or MetronInfo slot |
| `keywords` | used → `tags` (with the comic stories' keywords) |
| `story_set` | used → per-story mapping, see `Story` |
| `cover` | used → `cover_image_url` (kept as data; downloads blocked, see Covers) |

**`IssueOnly`** (search rows)

| Field | Status |
|---|---|
| `api_url` | used → candidate id |
| `series_name` | used → candidate series name/year; exact-name ordering |
| `descriptor` | used → candidate issue number |
| `publication_date` | used → candidate cover date (month when parseable, else year) |
| `variant_of` | used → variant rows skipped |
| `series` | used → candidate `series_external_id` (auto-split needs no detail probe) |
| `price`, `page_count` | deliberately unused at search: `IssueCandidate` has no slot. The apply reads them from the detail |

**`SeriesOverviewItem`** (`/api/series/{id}/overview/`)

| Field | Status |
|---|---|
| `issue_id` | used → candidate id |
| `number` / `descriptor` | used → candidate issue number (and page matching) |
| `publication_date`, `key_date` | used → candidate cover date |
| `on_sale_date` | deliberately unused at search: `IssueCandidate` has no store date. The apply maps it from the detail |
| `cover_url` | used → candidate `cover_image_url` (data; hashing skipped while the host is blocked) |
| `longest_story` | used → candidate name (story title). Its credits, characters, genre and synopsis have no candidate slot. The apply still needs `/api/issue/{id}/`, because the overview lacks ISBN, barcode, price, rating, editing, the cover story, keywords and variants |

**`Series`**

| Field | Status |
|---|---|
| `api_url` | used → GCD series id |
| `name` | used → `series_name`; exact-name ranking |
| `year_began` | used → `year_began` |
| `year_ended` | used → `year_end` |
| `publisher` | used → `publisher` name (one cached request) |
| `language` | used → `language_code` (lowercased ISO 639) |
| `country` | deliberately unused: Folio, ComicInfo and MetronInfo have no country field, and `language` already carries the locale signal |
| `active_issues` + `issue_descriptors` | used → issue index: `issue_count` (distinct numbers), splitter, overview page estimate, variant siblings |
| `publishing_format` | used → `series_type` (Metron vocabulary: `Ongoing Series`, `Limited Series`, `One-Shot`, `Trade Paperback`, `Hard Cover`, `Graphic Novel`, `Omnibus`, `Annual Series`), `format` (ComicInfo; none for ongoing), and the WP-5.6 matcher hint, which separates the Invincible TPB series from the ongoing one. Its `was …` prefix is deliberately not mapped to `series.status`: status is reconciled from `series.json` / `<Count>` by the scanner, and no provider writes it |
| `binding` | used → format fallback (`hardcover`; `trade paperback`) and hardcover upgrade of a collected format. `softcover` and `squarebound` alone are ignored as ambiguous |
| `color` | deliberately unused: `GenericMetadata` has no black-and-white slot, and ComicInfo `BlackAndWhite` is scanner-owned |
| `dimensions`, `paper_stock` | deliberately unused: physical attributes with no Folio, ComicInfo or MetronInfo field |
| `notes` | used → `GenericMetadata.notes` on the series detail (shown in the preview; the series apply has no series-notes column and never composes series notes into an issue's `<Notes>`) |

**`Publisher`**

| Field | Status |
|---|---|
| `name` | used → `publisher` |
| `api_url` | used → publisher id (cache key) |
| `country`, `year_began`, `year_ended`, `year_began_uncertain`, `year_ended_uncertain`, `year_overall_began`, `year_overall_ended`, `year_overall_began_uncertain`, `year_overall_ended_uncertain`, `notes`, `url`, `modified` | deliberately unused: Folio models a publisher as a name only (`publishers` table / `series.publisher`). There is no publisher-history, website or country field, and the 7-day name cache makes `modified` moot |
| `brand_count`, `indicia_publisher_count`, `series_count`, `issue_count` | deliberately unused: catalogue statistics with no Folio surface |

**`Story`** (inside `story_set` and `longest_story`)

| Field | Status |
|---|---|
| `type` | used → story selection: `comic story` for credits, characters, genres and synopsis; `cover` for `CoverArtist`. Text stories, ads and letters pages are ignored |
| `sequence_number` | used → story order |
| `page_count` | used → main-story pick (longest comic story, first on a tie) |
| `title` | used → `title` fallback (main story) |
| `script` / `pencils` / `inks` / `colors` / `letters` / `editing` | used → `Writer` / `Penciller` / `Inker` / `Colorist` / `Letterer` / `Editor` credits. Cover `pencils` and `inks` → `CoverArtist` |
| `characters` | used → `characters` + `teams` (first appearance, death) |
| `genre` | used → `genres` (title-cased, deduped) |
| `synopsis` | used → `description` (main story first, then other comic stories) |
| `keywords` | used → `tags` |
| `feature` | deliberately unused: the closest slot is ComicInfo `MainCharacterOrTeam`, which `GenericMetadata` doesn't model (it is carried through from `comic_info_raw`). Possible follow-up via the add-a-field recipe |
| `first_line` | deliberately unused: the opening line of dialogue is not a summary, and there is no slot for it |
| `job_number` | deliberately unused: a publisher-internal production code |
| `notes` | deliberately unused: per-story indexer notes. The issue's `notes` are already mapped, and stacking story notes would bloat `<Notes>` |

Tally: 11 paths (8 used, 3 unused). 93 fields across the six object
schemas (not counting the four pagination envelopes): 64 used, 29
deliberately unused. The unused fields are 16 `Publisher` catalogue
fields, 4 `Series` physical/locale fields, 4 `Story` fields, 2 `Issue`
indicia fields, 2 `IssueOnly` fields at search time, and the
overview's `on_sale_date` at search time.

Fixtures under `crates/server/tests/fixtures/gcd/` are recorded upstream
responses. Re-record with `curl 'https://www.comics.org/api/<route>?format=json'`
(anonymous is fine for a handful of calls at 30/h; keep to 1 req/s).

## Provider range detection ("Detect from providers")

Providers disagree on series boundaries. ComicVine lumps a run into one
volume; Metron and GCD split a legacy-renumbered relaunch into its own
series (GCD ends Fantastic Four (1961) at #416). The local series stays
whole and the exceptions live in `series_provider_range` (see CLAUDE.md,
"Provider series-boundary divergence"). Detection writes those rows.

**Entry points.** The Details-tab button
(`POST /api/series/{slug}/provider-ranges/detect`, admin, audited as
`admin.series.provider_range_detect`) runs
[`metadata/series_link.rs`](../../crates/server/src/metadata/series_link.rs)
for every provider. The post-apply hook (`jobs/metadata_apply.rs`,
manual series applies only) runs the detector alone for the providers
the apply matched.

**Resolving the provider series.** Only providers that can list a
series' issues can show a split (`MetadataProvider::enumerates_series_issues`:
Metron, GCD). For each one, in order (Metron first, because its
cross-reference row also carries the GCD id):

1. **Linked**: a series `external_ids` row for that source (a `user` row
   wins), or the latest applied run candidate.
2. **Bridge, cache** (free): a cached series detail of a provider we're
   linked to that lists the target id, or a cached target series whose
   identifiers list one of ours (used only when exactly one matches).
3. **Bridge, network**: Metron's curated cross-reference
   (`GET /api/series/?cv_id=` or `?gcd_id=`, one request,
   `find_series_by_cross_ref`). For GCD, the linked Metron series'
   detail `gcd_id` (7-day cache). An ambiguous cross-reference (more than
   one row) is ignored.
4. **Search**: the provider's series search, `PreFilter::from_library`
   (blacklist + hard year gate), then `matcher::score_series`. A
   candidate is **strict** when it is at least MEDIUM, its sanitized name
   equals the local name, its start year equals the local year, its
   publisher doesn't conflict (unknown is fine: Metron's list and a cold
   GCD cache carry none) and it has no format mismatch. It is used only
   when it is the single strict candidate **and** its own issue list
   carries at least half of the local numbered issues
   (`MIN_CONFIRM_OVERLAP`; free for GCD because the search fills the
   index cache, and reused by the detector). Everything else that buckets
   MEDIUM or better comes back as `needs_confirmation` candidates (top 3)
   and is **never written**. The admin's "Use this series" writes a
   `user` external id through the normal external-ids endpoint and runs
   detection again.

Ids found by steps 2–4 are recorded with
`writers::set_external_id_promoting` and `SetBy::Provider(attestor)`
(`metron` for a Metron cross-reference, the provider itself for a search
hit). The writer keeps the user-precedence rule. If another live series
already owns that provider id, nothing is written and the source reports
`no_series` with a note.

**Detecting the split**
([`metadata/auto_split.rs`](../../crates/server/src/metadata/auto_split.rs)):

- Gaps are computed over the **numerically numbered** local issues in
  numeric order. Annuals, letter suffixes (`14AU`) and vulgar fractions
  are counted (`uncovered_specials`) but never put in a range: a range
  bound like `"Annual 1"` can't be matched by `range_map::issue_in_range`.
  Runs are cut at every covered local issue and at every number the
  matched series lists between two uncovered local issues, so a range
  never swallows an issue the main series carries. Duplicate numbers
  count once.
- Per run (at most `MAX_GAPS_RESOLVED` = 3 per provider per click): one
  broad issue search for the run's first issue, candidates restricted to
  that number, up to 4 detail probes for a missing series id, then the
  candidate series' own issue list must carry at least half of the run.
  A same-numbered issue of an unrelated series is not enough.
- Existing ranges are never overwritten. An overlapping run is
  `already_mapped`. An automated range whose issues the matched series
  now lists itself (or that points at the matched series) is reported in
  `stale_ranges`, not deleted.
- A rate-limit or credentials failure stops that provider (`rate_limited`
  / `error`); rows written before it stay written and are listed. Other
  providers carry on.
- When two or more providers were scanned, `agreement` says whether they
  found the same uncovered runs. Disagreement is normal: each provider
  routes its own issues through its own rows.

**Cost per click** (cold caches): Metron 1 cross-reference + ⌈n/100⌉
enumeration pages + per run (1 search + ≤ 4 probes + 1 enumeration);
GCD 0 requests when bridged, else 1–4 search pages; enumeration 0–1;
per run 1–2 searches + 1 series. A click stops starting new providers
after `DETECT_TIME_BUDGET` (40 s) so it stays inside the 60 s JSON route
timeout.

**Where the rows are used.** Issue search narrowing and the year gate
(`orchestrator::run_issue_search`), the sidecar apply's series-identity
overlay (`apply_series_via_sidecar`), the coverage card
(`/provider-coverage`), the issue page's alternate-series list
(`api/issues.rs`), the folder-name health check
(`scanner/folder_checks.rs`), and the relationship engine (the `split`
continuation qualifier and the `provider_range` → `see_also` source).

## Adding a new provider

1. Implement `MetadataProvider` in `metadata/<name>.rs`. Look at
   `metron.rs` as the cleaner reference (CV's envelope handling is
   noisier); `gcd.rs` is the template for an upstream whose schema is
   unstable (untyped `Value` + alias reads).
2. Add the provider's auth credentials to the settings registry
   (`crates/server/src/settings/registry.rs`) + Config struct +
   `apply_overlay_row` (mirrors the metron entries).
3. Add the `Source::<Name>` variant + `Source::as_str` + `Source::label`
   + `Source::from_str` aliases + `canonical_url` template.
4. Append to `build_providers` in `orchestrator.rs` (priority order
   is positional) and add the `Source` arm to `apply::build_provider`,
   `api/admin_metadata.rs` (`provider_views` + `test_provider`),
   `budget::for_provider`, and the CSP `img-src` allowlist for its
   cover host. Web: `ProviderConfigForm` + `ProvidersTab` docs link.
5. Add a wiremock-backed integration test under
   `crates/server/tests/metadata_<name>.rs` mirroring `metadata_apply.rs`.
6. The matcher + apply + diff + UI surfaces auto-work since they
   speak only `GenericMetadata` + `Source` + `Identifier`.

## Adding a new metadata field

1. Add the variant to `MetadataField` enum + `SCALAR_FIELDS` const +
   `key()` match + `from_str` parser
   (`crates/server/src/metadata/field.rs`). The
   `key_round_trip_for_every_variant` test catches forgotten arms.
2. Add to the appropriate `SeriesUpdates` / `IssueUpdates` struct in
   `apply.rs`.
3. Add a `decide_str` / `decide_i32` / `decide_scalar` call in
   `apply_series` or `apply_issue`.
4. Add a `push_scalar` call in `diff::compute_series_diff` or
   `compute_issue_diff` for the preview pane.
5. If junction-shaped (one entity → many people / characters / etc.),
   write the junction reconcile helper in `writers.rs`.

## Reviewer heuristics — common mistakes

- **Don't hand-write `serde_json::json!({"error": …})` envelopes for
  metadata errors.** Route everything through `api::error(status,
  code, message)`. The error codes already in use:
  `metadata.candidate_not_found`, `metadata.run_not_found`,
  `metadata.no_providers`, `metadata.invalid_scope`,
  `metadata.queue`, `metadata.provider`.

- **Adding a new field on the issue / series row?** Update the M0
  schema migration's CSV rebuild + the writer's
  `rebuild_issue_csv_cache` + every test seed. The CSV columns are
  denormalized read-cache; they MUST be rebuilt on junction writes
  or they drift.

- **Touching the matcher weights?** The `metadata.auto_apply_threshold`
  setting (default 95) is calibrated against the current weight
  table. Rebalancing weights without revisiting the threshold breaks
  HIGH-bucket semantics across every existing `metadata_run_candidate`
  row.

- **Provider responses can carry NULL for any field.** The matcher
  treats most NULL cases as half-credit so a sparse but correct
  candidate isn't unfairly penalized vs a complete but wrong one.
  Don't change that without measuring the effect on existing match
  bucketing.

- **The diff endpoint's `selected_fields` is the source of truth for
  per-field opt-in.** Apply's `should_apply` reads it BEFORE the
  user-precedence rule. Adding a new gating predicate goes there,
  not in the writers.

- **Phash bonus is bounded.** Don't promote it above 10 without
  re-calibrating the HIGH threshold — at 15+ a single perfect cover
  match can rescue a flagrantly wrong name match.

## Files

- [`crates/server/src/metadata/`](../../crates/server/src/metadata/) — the whole subsystem
- [`crates/server/src/api/metadata_search.rs`](../../crates/server/src/api/metadata_search.rs) — per-entity HTTP routes
- [`crates/server/src/api/admin_metadata.rs`](../../crates/server/src/api/admin_metadata.rs) — admin dashboard routes
- [`crates/server/src/jobs/metadata_search.rs`](../../crates/server/src/jobs/metadata_search.rs) + [`metadata_apply.rs`](../../crates/server/src/jobs/metadata_apply.rs) — apalis workers
- [`web/components/library/MetadataMatchDialog.tsx`](../../web/components/library/MetadataMatchDialog.tsx) — the dialog
- [`web/components/library/MetadataPreviewPane.tsx`](../../web/components/library/MetadataPreviewPane.tsx) — M5 diff view
- [`web/components/admin/metadata/`](../../web/components/admin/metadata/) — admin tabs

[provider]: ../../crates/server/src/metadata/provider.rs
[source-fromstr]: ../../crates/server/src/metadata/identifier.rs
[build-providers]: ../../crates/server/src/metadata/orchestrator.rs

## Series identity edits (roadmap WP-2.3)

`PATCH /series/{slug}` accepts the identity fields — `name`, `year`,
`volume`, `publisher`, `imprint`, `age_rating`, `total_issues`,
`language_code` — alongside the older `status` / `summary` / direction
fields. Each touched field is written and pinned with a
`field_provenance` row (`set_by='user'`): `Title`, `YearBegan`, `Volume`,
`Publisher`, `Imprint`, `AgeRating`, `TotalIssues`, `LanguageCode`.

Three readers honour those pins:

- the provider apply (`should_apply` / `user_pinned`) skips pinned
  fields unless an admin forces the apply;
- the scanner's series reconcile (`reconcile_status::apply_reconciled_status`)
  leaves a pinned name / publisher / volume / issue count alone even when
  a `series.json` sidecar carries another value;
- the scanner's ingest already keeps issue-level pins.

Renaming never re-homes a folder: the scanner resolves a folder to its
series by `match_key`, then `folder_path` (identity.rs tiers 1–2), so the
name / year change only affects display and provider matching. The
`normalized_name` column is refreshed with the new name.

## Rescan precedence (roadmap WP-2.5, decision D4)

For a library **without** archive writeback the database is the record
and the archive is a source. On every rescan the scanner applies the same
attribution ladder the provenance writer already enforced — **user >
provider > file** — to the columns themselves:

- a scalar whose `field_provenance` row is `user` or a provider name is
  left alone (`process.rs` `protected()`); only file-tier or unattributed
  columns are refreshed from ComicInfo / MetronInfo;
- junction tables owned by a user edit or a provider apply (credits,
  characters, teams, locations, genres, tags) are skipped by the rollup
  (`metadata_rollup::replace_issue_metadata_skipping`), so the provider's
  person ids and ordinals survive and nothing churns on each scan;
- a file-tier external id (`set_by=comicinfo|metroninfo`) never replaces a
  provider-set row (`writers::put_external_id` → `KeptProviderValue`);
- `review` has no provenance slot and no provider writes it, so it stays
  file-owned.

Edit endpoints write their user pins in the same transaction as the row
update (`api/issues.rs::update_issue_with_user_pins`), so a pin can no
longer be lost while the edit lands.

Libraries **with** writeback are the inverse model: the archive is
canonical and the rescan re-ingests what the composer wrote. There the
provider tier protects a column only while the archive's XML does **not**
carry its value — the provenance row is newer than the issue's
`last_sidecar_rewrite_at`, or the issue was never sidecar-rewritten (a
DB-direct fallback for a refused CBR/CB7, or an apply from before the
library enabled writeback). User pins protect in both models. See
[metadata-sidecar-writeback.md](metadata-sidecar-writeback.md#rescan-ingest-of-provider-values).
