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
| `metadata_cache` | `(provider, entity, external_id)` | normalized `GenericMetadata` JSON + `schema_version` + `etag` / `last_modified` validators | 24 h issue / 168 h series (settings) |
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
third provider and the least like the others:

- **Endpoints.** Series search is `GET /api/series/name/{name}/[year/{year}/]`
  (`icontains`, sorted by name; Folio queries the exact-year route
  first, then the name-only page to fill). Issue search is
  `GET /api/series/name/{name}/issue/{number}/[year/{key-date year}/]`.
  Details are `/api/series/{id}/`, `/api/issue/{id}/`,
  `/api/publisher/{id}/`. Relations are hyperlinks; ids are parsed out
  of them.
- **Tolerant parsing.** GCD declares its API fields unstable, so the
  client reads `serde_json::Value` through alias lists (`str_field`,
  `int_field`, `entity_id`) instead of typed structs. A renamed field
  falls through to its alias (or `None`), a re-typed scalar is coerced,
  an unknown field is ignored, an item with no recoverable id is
  skipped, and a missing `results` envelope reads as "no results".
  Only a non-JSON body is `InvalidResponse`.
  `tests/gcd_client.rs::renamed_and_unknown_fields_still_parse` pins it.
- **Free-text credits.** GCD stores credits per *story* as text
  (`"Stan Lee (signed as …); Sol Brodsky ? (see notes)"`). The client
  maps `comic story` script/pencils/inks/colors/letters/editing onto
  the ComicInfo roles, the `cover` story's pencils/inks onto
  `CoverArtist`, and issue-level `editing` onto `Editor` only when the
  annotation names an editor (production staff are dropped).
  Placeholders (`None`, `?`, `typeset`, `various`) and uncertain
  credits (trailing `?`) are omitted. Characters parse
  `Team [Member [Alter ego]; …]; Character (first appearance)`; a
  bracket group with several members makes its head a **team**.
- **Covers + search cost.** Search payloads carry no cover, so the
  client hydrates up to 4 issue details on the narrowed path (folding
  a variant's cover into its parent's `alternate_cover_urls`) and 2 on
  the broad path. Series summaries + publisher names are cached in
  Redis (`metadata:gcd:{series,publisher}:<id>`, 7 days) so an issue
  fetch costs one request.
- **Splitter.** `list_series_issue_numbers` reads the series detail's
  `issue_descriptors` (one request; variants deduped). GCD splits
  e.g. Fantastic Four (1961) at #416, so auto-split maps the legacy
  #500+ run onto its own GCD series.
- **CSP.** Cover images come from `https://files1.comics.org`, which is
  in the `img-src` allowlist (`middleware/security_headers.rs`).
- **License.** CC BY-SA 4.0; the canonical comics.org links drive the
  attribution footer. The composer's `Notes` audit line stays
  CV/Metron-only (its wording is the CC-BY-NC-SA one).

Fixtures under `crates/server/tests/fixtures/gcd/` are recorded upstream
responses; re-record with `curl 'https://www.comics.org/api/<route>?format=json'`
(anonymous is fine for a handful of calls — 30/h).

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
canonical and the rescan re-ingests what the composer wrote (see
[metadata-sidecar-writeback.md](metadata-sidecar-writeback.md)).
