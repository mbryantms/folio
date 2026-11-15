# Entity profiles and library insights

The landing pages for characters, teams, locations and story arcs
(`/characters/{slug}` etc.), and the creator page (`/creators/{slug}`,
above its role rails), have two blocks above the issue and series
grids: a **provider-filled profile** (the "wiki" block) and **In your
library** statistics. Publishers keep the short fields only.

## Provider profile

One fetch per entity, run by the `entity_enrich` job
(`crates/server/src/jobs/entity_enrich.rs`) and implemented in
`crates/server/src/metadata/entity_profile.rs`.

**When it runs.** The first **admin** `GET /<kind>/{slug}` that finds
no `entity_profile` row (or a row whose fetch was requested more than 30
minutes ago and never completed) stamps `requested_at` and pushes the
job. The page reports `enrichment = pending` and the web polls the
header every 4 s until it flips to `done` / `failed`. An admin's
**Refresh** (`POST /<kind>/{slug}/profile/refresh`, audited as
`admin.entity.refresh_profile`) forces a new fetch. A reader's page view
never queues a fetch (it would spend the admin's provider budget, and
any reader could drive provider traffic by walking entity pages): it is
served the cached profile, `pending` while a fetch an admin queued is in
flight, else `unavailable`. There is no bulk "fill every character" job
on purpose: ComicVine allows 200 requests per resource per hour, so
profiles are fetched as admins view pages.

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

## Creators

A creator (`person` row, `ProfileKind::Person`, roadmap WP-9.9) gets the
same profile from the same job, bucket and lazy model: an admin's first
`GET /creators/{slug}` queues `entity_enrich`, a reader is served the
cache, and `POST /creators/{slug}/profile/refresh` (admin, audited as
`admin.entity.refresh_profile` with `target_type = person`) forces a
fetch. `CreatorDetailView` carries `image_url`, `description`, `aliases`,
`profile`, `links`, `enrichment` and `enrichment_error`, shaped like
`EntityDetailView`'s.

- **Metron** `GET /api/creator/?name=` then `/api/creator/{id}/`: name,
  birth, death, description, image, aliases, `cv_id`, `gcd_id`. There is
  no issue list for a creator, so no count request.
- **ComicVine** `GET /person/4040-{id}` (search: `/people/?filter=name:`),
  in the pooled `entities` bucket: deck, wiki description, image, aliases,
  gender, birth, death, country, hometown, website, email, created
  characters, series credits and story-arc credits. The per-issue credit
  list is not requested: it runs to megabytes for a long career. The
  record's issue count is requested but ComicVine returns it empty for a
  person (seen on dev, 2026-11-15), so `provider_counts` stays empty and
  the series credits are the measure of a career.

`Facts` gains `born`, `died`, `country`, `hometown`, `website`, `email`,
`characters_created`, `series_credits` and `story_arc_credits`. The owner chose to store
and show everything ComicVine holds, email and hometown included.

**Which record.** `person` is unique by normalized name, and two real
creators can share one. So:

1. A provider id the creator already holds is fetched as it is. An apply
   links it (roadmap WP-9.8; see
   [metadata-sidecar-writeback.md](metadata-sidecar-writeback.md)), and so
   does the Creator links backfill.
2. With no id, the name is searched, exact matches only (case and spacing
   aside). One match is taken.
3. Several same-named records are opened, at most three per provider
   (`MAX_PERSON_CANDIDATES`), and the ComicVine record whose series credits
   overlap the creator's local series most is chosen (`pick_person`: by
   ComicVine volume id, else by series name; a tie goes to the record
   with more series credits). When none of them shares a series with the
   creator, none is linked and the fetch records why: a guess would stay
   as the creator's record. Metron's creator record lists no credits, so same-named Metron
   creators are resolved through the ComicVine record each one names; with
   no ComicVine answer and several Metron creators, none is linked.

A fetch therefore costs one detail request per provider for a linked
creator, and a search plus one to three details for an unlinked one.

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

**Creators** (roadmap WP-9.10): `GET /creators/{slug}/insights`
(`creators::insights`) answers with the same `EntityInsightsView`, counted
over the issues the creator is credited on that the caller can see
(library access and the age-rating cap, on the issue and its series):
totals, the caller's read count, publisher split, issues by decade, top
series, the characters those issues feature most and the creators
credited beside them most often. `teams`, `locations`, `arcs` and
`earliest` are empty for a creator. It runs
`creators::INSIGHTS_STATEMENTS` (6) statements whatever the creator's
size, reading the credits by `issue_credits.person_id` (the series rollup
links every credit). A creator with no visible issue credit is a 404, like
an unknown slug; a creator known only from series-level credits therefore
has a page and no statistics block.

Visibility follows the rule in
[`docs/architecture/auth-model.md`](../architecture/auth-model.md#entity-surfaces):
for a caller with an age cap the entity must be linked from an issue that
caller can see, otherwise every route under `/<kind>/{slug}` answers 404
like an unknown slug. The provider profile is shown whenever the entity
itself is visible.

## Web

- `web/components/library/EntityProfile.tsx` — the profile block and
  the wiki sections (lead + first section open, the rest as chips).
- `web/components/library/EntityInsights.tsx` — the statistics block.
- `useEntityDetail` polls while pending; `useRefreshEntityProfile` is
  the admin Refresh.
- Creators: `web/components/library/CreatorProfile.tsx` puts
  `EntityProfile` (kind `creators`: born, died, hometown, country, gender,
  website, email, credit counts, a "Characters created" chip row),
  and `CreatorInsights` above the role rails on the overview of the
  creator page, and `EntitySections` (`CreatorBiography`) below them: a
  creator's write-up is often one long unheaded text that opens whole. `useCreatorDetail` polls while pending;
  `useRefreshCreatorProfile` is the admin Refresh. Website and email are
  links only when `factHref` accepts them (`http(s)` URLs, plain
  addresses).
- `web/components/library/CreatorAvatar.tsx` — the round portrait with the
  coloured monogram as its fallback (no image yet, or one that fails to
  load). `CreatorListItem.image_url` and `PersonHit.image_url` feed it on
  the `CreatorsIndex` cards; the `PersonPreview` hover card reads the
  detail payload.

## Tests

- `crates/server/tests/entity_pages.rs` — `profile_fetch_merges_metron_and_comicvine`
  (wiremock both providers), `profile_is_queued_on_first_view_and_refreshed_by_admin`,
  `reader_view_queues_no_profile_fetch`,
  `insights_report_library_stats_under_the_acl`.
- `crates/server/tests/entity_pages.rs`, creators —
  `creator_profile_merges_metron_and_comicvine`,
  `creator_profile_is_queued_by_an_admin_view_only_and_refresh_is_audited`,
  `same_named_creators_are_resolved_by_local_series_overlap`.
- `crates/server/src/metadata/entity_profile.rs` — section parser unit
  tests, `same_named_creators_are_told_apart_by_local_series`.
- `web/tests/library/entity-profile.test.tsx`.
- Creators: `entity_pages.rs::creator_insights_count_only_visible_issues`
  (a capped reader's numbers), `perf_regressions.rs` (`creator_insights_guard`:
  the same statement count for a creator on 100 issues and on 2),
  `web/tests/library/creator-profile.test.tsx`.
