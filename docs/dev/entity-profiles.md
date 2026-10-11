# Entity profiles and library insights

The landing pages for characters, teams, locations and story arcs
(`/characters/{slug}` etc.) have two blocks above the issue and series
grids: a **provider-filled profile** (the "wiki" block) and **In your
library** statistics. Publishers keep the short fields only.

## Provider profile

One fetch per entity, run by the `entity_enrich` job
(`crates/server/src/jobs/entity_enrich.rs`) and implemented in
`crates/server/src/metadata/entity_profile.rs`.

**When it runs.** The first `GET /<kind>/{slug}` that finds no
`entity_profile` row (or a row whose fetch was requested more than 30
minutes ago and never completed) stamps `requested_at` and pushes the
job. The page reports `enrichment = pending` and the web polls the
header every 4 s until it flips to `done` / `failed`. An admin's
**Refresh** (`POST /<kind>/{slug}/profile/refresh`, audited as
`admin.entity.refresh_profile`) forces a new fetch. There is no bulk
"fill every character" job on purpose: ComicVine allows 200 requests per
resource per hour, so profiles are fetched as pages are viewed.

**Provider order.**

1. **Metron** (characters, teams, story arcs; it has no location
   records): `GET /api/<kind>/?name=` picks the exact-name hit, then the
   detail, then one `issue_list/` page for the issue count. Metron's
   detail carries `cv_id` and `gcd_id`, so a Metron match resolves the
   ComicVine record without a search and links all three providers.
2. **ComicVine** (all four kinds): a direct detail fetch by the known id,
   else an exact-name search where the hit with the most appearances
   wins. Its requests count against the pooled `entities` resource
   bucket (`rate_limit::ComicVineResource::Entities`), so page views
   never compete with the matcher's volume / issue budgets.

**What is written.**

| Where | Fields | Rule |
| --- | --- | --- |
| entity row (`character` / `team` / …) | `description`, `image_url`, `aliases`, `real_name` | `writers::update_entity_display`: ComicVine's deck / image / aliases win, Metron fills gaps; a missing value never blanks an existing one |
| `external_ids` | metron, comicvine, gcd ids | `writers::set_external_id`, `set_by = provider` |
| `entity_profile` | `summary` (Metron's description, else the deck), `sections`, `facts`, `provider_counts`, `sources`, `fetched_at`, `error` | one row per `(entity_type, entity_id)` |

`sections` is ComicVine's wiki description split at `<h2>` / `<h3>` into
plain-text paragraphs (`sections_from_html`; tables, figures, scripts
dropped). `facts` is the fact sheet (`Facts`: real name, gender,
birthday, publisher, first appearance, creators, teams, members, powers,
universes). A fetch that matched nothing records why in `error` with
`fetched_at` set, so the page shows `failed` and does not retry on every
view; Refresh retries.

## Library insights

`GET /<kind>/{slug}/insights` (`entity_pages::insights_handler`)
aggregates over the same ACL-filtered `(issue, series)` hit set the
grids use (`issue_hits_one`): totals, the caller's read count, publisher
split, issues by decade, top series, co-appearing characters / teams /
locations / arcs (FK or scanner-name resolved, the entity itself
excluded), most frequent creators, and the first ten issues in the
entity's reading order as issue cards. No provider is involved, so this
block works for every entity today.

## Web

- `web/components/library/EntityProfile.tsx` — the profile block and
  the wiki sections (lead + first section open, the rest as chips).
- `web/components/library/EntityInsights.tsx` — the statistics block.
- `useEntityDetail` polls while pending; `useRefreshEntityProfile` is
  the admin Refresh.

## Tests

- `crates/server/tests/entity_pages.rs` — `profile_fetch_merges_metron_and_comicvine`
  (wiremock both providers), `profile_is_queued_on_first_view_and_refreshed_by_admin`,
  `insights_report_library_stats_under_the_acl`.
- `crates/server/src/metadata/entity_profile.rs` — section parser unit tests.
- `web/tests/library/entity-profile.test.tsx`.
