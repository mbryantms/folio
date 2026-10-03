# Load testing and query-plan baseline

This is the 50,000-issue baseline for Folio's list, filter and sort paths
(WP-3.6; audit R25, OP-6 and AR-1). It covers:

1. **`just perf-explain`**, which records `EXPLAIN (ANALYZE, BUFFERS)` for
   the hottest list endpoints against a stress-scale library and fails when
   a plan seq-scans a large table.
2. **The recorded plans**: before and after for every plan this work
   changed, and the current plan for the ones it left alone.
3. **The AR-1 projection audit** of `issue::Entity::find()` call sites.
4. **An `oha` recipe** for HTTP load, mapped to the spec §18.3 scenarios.
5. **The M7 surfaces at 50,000 issues** (WP-8.3): similar series,
   relationships, same universe, arc tie-ins, the suggestion queue and a
   full `relationship_suggest` job run.

Scanner throughput is documented separately in
[`scanner-perf.md`](scanner-perf.md).

## `just perf-explain`

```sh
just perf-explain                        # the full run: fixture, scan, plans (~5 min cold)
PERF_KEEP=1 just perf-explain            # keep the Postgres + Redis containers afterwards
PERF_PG_CONTAINER=folio-perf-pg-… \
PERF_REDIS_CONTAINER=folio-perf-redis-… \
PERF_KEEP=1 just perf-explain            # rerun against the seeded DB (no scan, ~1 min)
PERF_OHA=1 just perf-explain             # also run an oha load pass (see below)
```

[`scripts/perf/perf-explain.sh`](../../scripts/perf/perf-explain.sh) runs
these steps:

1. **Fixture.** Runs `fixtures/build.py --scale stress --series 2500 --rich`,
   which writes 2,500 series × 20 issues = **50,000 CBZs** to
   `fixtures/library-stress-2500x20/` (gitignored, ~200 MB, ~40 s). `--rich`
   adds deterministic writer, penciller, genre, tag, character, title,
   summary, age-rating and publisher values, drawn from pools of eight
   publishers, 400 writers and 1,500 characters. Series get varied names
   ("Crimson Harbor", "Silent Vector", …) so trigram and full-text search
   selectivity looks like a real library rather than 2,500 near-identical
   "Series NNNN" names. Without `--rich` the stress set is unchanged, so
   `just perf-scan` numbers stay comparable.
2. **Throwaway services.** Starts `postgres:18-alpine` (with `auto_explain`
   preloaded) and `redis:8-alpine` on free loopback ports, named
   `folio-perf-{pg,redis}-<ts>-<pid>`. The run never touches the dev
   services on 5432 and 6380. On exit it removes only the containers it
   started, and only by exact name.
3. **Real ingest.** Boots the server binary against those services,
   registers the admin and four other readers, creates a library over the
   fixture with thumbnails off, and scans it. A debug build takes about
   200 s. The data therefore comes from the scanner itself: series, issues,
   junctions, credits, `field_provenance` and `search_doc`.
4. **Per-user activity.**
   [`scripts/perf/seed_activity.sql`](../../scripts/perf/seed_activity.sql)
   adds deterministic reading data:
   - The caller's progress: finished prefixes on 20 % of series and 250
     in-progress issues (about 3,300 rows).
   - The same shape for the other four readers, so the caller owns about
     20 % of `progress_records`.
   - A 640-entry CBL (600 matched, 40 missing) with its saved view.
   - A publisher + year filter view.
   - A 300-entry collection.

   It finishes with `VACUUM ANALYZE`.
5. **Plans from the real SQL.** Sets `auto_explain.log_min_duration = 0`
   with `log_analyze`, `log_buffers` and `log_nested_statements` on, then
   drives every endpoint in
   [`scripts/perf/endpoints.txt`](../../scripts/perf/endpoints.txt) over
   HTTP. Each endpoint gets one warm-up request, then a measured request
   bracketed by marker statements. The plans are for the exact statements
   the handlers emit, with real bind values, so there is no hand-copied SQL
   to drift.
6. **Report.**
   [`scripts/perf/explain_report.py`](../../scripts/perf/explain_report.py)
   splits the Postgres log per endpoint into
   `perf-out/explain-<ts>/plans/<label>.txt` and writes `summary.md`.
   - **Gate:** exit 1 when any plan seq-scans a table with ≥ 5,000 rows
     (`PERF_SEQSCAN_MIN_ROWS`). That covers `issues`, its junctions,
     `field_provenance`, `progress_records` and anything else that grows
     with the issue count.
   - **Info:** seq scans on tables with 1,000–4,999 rows are listed but
     don't fail the run. At this scale that is only `series` (2,500 rows,
     about 150 pages), where the planner can legitimately prefer a scan.

7. **Phase 2, the M7 set** (WP-8.3; `PERF_M7=0` skips it). See
   [M7 surfaces at 50,000 issues](#m7-surfaces-at-50000-issues-wp-83).
   [`scripts/perf/seed_relationships.sql`](../../scripts/perf/seed_relationships.sql)
   layers arcs, universes, series groups, AlternateSeries and curated
   links onto the catalogue. The run then times three full
   `relationship_suggest` job runs, bulk-accepts 501 suggestions through
   the API, and drives
   [`scripts/perf/endpoints-m7.txt`](../../scripts/perf/endpoints-m7.txt)
   the same way as phase 1. A third column `cold` there skips the warm-up
   request, so similar series is measured on an in-process cache miss.
   - `job-*` labels (whole-library batch work) never fail the gate; their
     seq scans are listed with `[batch]`.
   - [`scripts/perf/expected_seqscans.txt`](../../scripts/perf/expected_seqscans.txt)
     holds reviewed `label | table | reason` exceptions, listed as
     `[expected]`.

`PERF_SERIES` and `PERF_ISSUES_PER_SERIES` resize the library (phase 2's
seed assumes the default 2,500 series).
`PERF_SERVER_BIN` runs a prebuilt binary instead of building the working
tree. The "before" numbers below came from an `origin/main` build run
against the same database.

### Dataset at stress scale

| table | rows | size (heap + indexes) |
|---|---:|---:|
| `field_provenance` | 550,019 | 147 MB |
| `issue_characters` | 150,000 | 38 MB |
| `issue_credits` | 100,000 | 42 MB |
| `issue_genres` | 95,000 | 23 MB |
| `issues` | 50,000 | 172 MB (≈ 2.3 KB/row; `comic_info_raw` alone ≈ 1 KB even on 2-page fixtures) |
| `issue_tags` / `issue_paths` | 50,000 | 12 / 38 MB |
| `series_credits` | 52,122 | 14 MB |
| `progress_records` | 20,235 | 14 MB |
| `series` | 2,500 | 3.4 MB |

## The top ten

These are the most-hit list, filter and sort surfaces, plus variants that
exercise a second plan shape (1b, 6b, 10b). The rows in `endpoints.txt`
below the top ten are secondary list paths, recorded for completeness.

| # | label | request |
|---|---|---|
| 1 | `series-browse-name` (+ `-page2` keyset) | `GET /api/series?library=…&limit=50` |
| 2 | `series-filter-sort` | `GET /api/series?publisher=…&year_from=2005&year_to=2015&genres=Horror&sort=year&order=desc` |
| 3 | `series-recent` | `GET /api/series?sort=created_at&order=desc&limit=24` |
| 4 | `series-search` | `GET /api/series?q=crimson` |
| 5 | `series-issues` | `GET /api/series/{slug}/issues?limit=50` |
| 6 | `issues-browse-recent` (+ `issues-filter-writer`) | `GET /api/issues?sort=created_at&order=desc` / `?writers=Writer 042&sort=year` |
| 7 | `issues-search` | `GET /api/issues/search?q=phoenix` |
| 8 | `continue-reading` | `GET /api/me/continue-reading` |
| 9 | `on-deck` | `GET /api/me/on-deck` |
| 10 | `saved-view-results` (+ `cbl-entries`) | `GET /api/me/saved-views/{id}/results` / `GET /api/me/cbl-lists/{id}/entries` |

### Results

"Σ SQL" is the total execution time of every statement the request ran,
as measured by `auto_explain` with per-node timing on, which inflates it.
"Wall" is the debug-build HTTP round trip on a shared, busy workstation.
Treat both as relative numbers.

| endpoint | before: Σ SQL ms | before: seq scans | after: Σ SQL ms | after: seq scans |
|---|---:|---|---:|---|
| series-browse-name | 4.1 | — | 4.7 | — |
| series-browse-page2 | 3.8 | — | 3.3 | — |
| series-filter-sort | 4.1 | series (info) | 6.3 | — |
| series-recent | 3.0 | series (info) | 2.0 | — |
| series-search | 10.3 | series (info) | 2.7 | — |
| series-issues | 1.1 | — | 0.2 | — |
| issues-browse-recent | **57.9** | **issues** | 4.3 | — |
| issues-filter-writer | **119.8** | **issues** | 0.4 | — |
| issues-search | **46.7** | **issues** | 17.6 | — |
| continue-reading | 2.3 | — | 3.3 | — |
| on-deck | **34.4** | **issues**, series (info) | 17.8 | series (info) |
| saved-view-results | 4.6 | series (info) | 2.8 | — |
| cbl-entries | 2.7 | — | 0.7 | — |
| *series-detail* | 1.2 | — | 0.9 | — |
| *recent-issues-rail* | **50.1** | **issues**, series (info) | 1.5 | — |
| *collection-entries* | 3.5 | — | 1.6 | — |
| *issue-next-up* | 1.2 | — | 0.3 | — |
| *series-resume* | 0.7 | — | 0.1 | — |

Before this work, five plans seq-scanned the 50,000-row `issues` table.
None do now, and the gate passes. One info-level scan remains, in
**on-deck**: `Seq Scan on series` as the build side of a hash join that
keeps 250 of 2,500 rows. The planner is right to pick it: a 150-page scan
(0.3 ms) is cheaper than 250 index probes, and it switches to probes on its
own as the ratio drops. The few-hundred-microsecond swings between runs on
unchanged plans (series-browse-name, continue-reading) are timing noise;
the plan shapes are identical.

A clean end-to-end rerun of the final code passed the gate: fresh
containers, a new scan, `PERF_OHA=1`. That run's planner also used one more
info-level `series` scan, in `series-browse-name`'s hydrate. Which
2,500-row series scans appear varies with statistics sampling. Its `oha`
pass (debug build, 16 connections, 5 s per endpoint, same busy host)
achieved these request rates with a 100 % success rate:

| endpoint | req/s | endpoint | req/s |
|---|---:|---|---:|
| series-browse-name | 817 | issues-search | 459 |
| series-filter-sort | 931 | continue-reading | 1,518 |
| series-search | 1,120 | on-deck | 215 |
| series-issues | 2,065 | saved-view-results | 802 |
| issues-browse-recent | 1,176 | cbl-entries | 1,008 |
| issues-filter-writer | 1,441 | recent-issues-rail | 1,717 |

`COUNT(*)` for the first-page `total` is inherently O(active issues). It is
now an index-only scan over the 5 MB `issues_active_created_idx` instead of
a scan of the 65 MB heap: 4 ms instead of 26 ms here, roughly 40 ms at
500,000 issues. The filtered counts use the same indexes as the lists they
belong to.

## What changed

All index changes are in migration
[`m20270215_000001_list_query_indexes`](../../crates/migration/src/m20270215_000001_list_query_indexes.rs).

| change | kind | fixes |
|---|---|---|
| `issues_active_created_idx` `(created_at DESC, id DESC) WHERE state='active' AND removed_at IS NULL` | index | issues-browse-recent list + count, recent-issues-rail walk |
| `issues_active_id_series_idx` `(id) INCLUDE (series_id) WHERE active` | index | on-deck and continue-reading progress→issue joins (index-only) |
| `folio_issue_facet_keys(13 CSV cols)` + `issues_facet_keys_gin` | function + GIN | every cross-library issue facet (`writers=`, `genres=`, `characters=`, …) |
| `series_created_idx`, `series_updated_idx` | index | "Recently added / updated" series rails |
| `series_publisher_idx` | index | publisher facet on the series grid, `publisher` saved-view predicate |
| issue search `A OR B` → `id = ANY(ARRAY(A UNION B))` | query (`issues::issue_search_condition`) | issues-search, `/issues?q=` |
| series search, same rewrite | query (`series::list`) | series-search |
| recent-issues rail: keyset walk instead of `ROW_NUMBER()` over the library | query (`rails::recent_issues`) | recent-issues-rail |
| `IssueCardRow` projection on list paths | projection (see the AR-1 audit) | every issue-card list |

**Facet semantics are unchanged.** The old predicate was
`EXISTS (unnest(regexp_split_to_array(col, CASE WHEN col LIKE '%;%' THEN ';' ELSE ',' END)) … lower(trim(piece)) = ANY($1))`,
evaluated row by row. `folio_issue_facet_keys` computes the same
`lower(trim(piece))` keys, prefixed `<column>:`, and the filter becomes
`folio_issue_facet_keys(...) && ARRAY['writer:…']`. Empty pieces are
dropped because a facet value is never empty. The regression test
`filter_options::issues_cross_library_facet_csv_matching_rules` pins the
rules: `;`-first splitting, trim and case-folding, whole-piece equality,
any-of within a facet, and AND across facets.

**Search rewrites are set-equivalent.** `A OR B ≡ id ∈ A ∪ B`. Each arm now
uses its own GIN index (`search_doc` or the series trigram index), and the
outer query probes the primary key or active-set index. For search, rank
sorting stays O(matches), because every match has to be ranked before the
top 20 can be picked.

**The recent-issues rail keeps the same result set.** It now walks the
active set newest-first in 128-row keyset batches, using
`(created_at, id) < (…)` as an index condition, and applies the three-per-
series cap in Rust. A series appears in the global walk in the same
`created_at DESC, id DESC` order the old window function ranked by, so the
cards are identical. `rails::recent_issues_walks_past_a_scan_flood` covers
a 140-issue single-series flood that crosses a batch boundary.

### Plans, before and after

The excerpts are condensed: cost estimates, buffer lines and bind literals
are trimmed. Full plans are in `perf-out/explain-<ts>/plans/` after a run.

**issues-browse-recent: count and list**

```text
before  Aggregate  [25.9 ms]
          ->  Seq Scan on issues  [rows=50000]  Filter: removed_at IS NULL AND state='active'
        Limit  [30.5 ms, rows=51]
          ->  Gather Merge -> Sort (created_at DESC, id DESC) top-N
                ->  Parallel Seq Scan on issues  [rows=16667 ×3]
after   Aggregate  [4.2 ms]
          ->  Index Only Scan using issues_active_created_idx  [rows=50000, heap fetches 0]
        Limit  [0.04 ms, rows=51]
          ->  Index Scan using issues_active_created_idx on issues  [rows=51]
```

**issues-filter-writer** (`writers=Writer 042`, year sort)

```text
before  Limit  [57.2 ms]
          ->  Sort (year IS NULL, year DESC, id DESC)
                ->  Seq Scan on issues  [rows=140, removed 49,860]
                      Filter: … AND EXISTS(SubPlan: Function Scan on unnest piece [×50000])
after   Limit  [0.19 ms]
          ->  Sort (year IS NULL, year DESC, id DESC)
                ->  Bitmap Heap Scan on issues  [rows=140]
                      Recheck Cond: folio_issue_facet_keys(genre, …, locations) && '{writer:writer 042}'
                      ->  Bitmap Index Scan on issues_facet_keys_gin  [rows=140]
```

**issues-search** (`q=phoenix`, 1,614 matches)

```text
before  Limit  [45.7 ms]
          ->  Sort (ts_rank_cd DESC) top-N
                ->  Seq Scan on issues  [rows=1614, removed 48,386]
                      Filter: … AND (search_doc @@ 'phoenix' OR series_id = ANY(hashed SubPlan: trigram series))
after   Limit  [17.2 ms]
          InitPlan: HashAggregate(Append(
                      Bitmap Heap Scan on issues (Bitmap Index Scan on issues_search_doc_gin) [rows=1614],
                      Nested Loop(series trigram bitmap → issues_series_slug_uniq) [rows=0]))
          ->  Sort (ts_rank_cd DESC) top-N
                ->  Bitmap Heap Scan on issues  [rows=1614]
                      ->  Bitmap Index Scan on issues_active_id_series_idx  (id = ANY(InitPlan))
```

**on-deck: candidate series**

```text
before  … Hash Join (s.id = started.series_id)
            ->  Seq Scan on series s  [0.3 ms, rows=2500]                       (info)
            ->  HashAggregate (i.series_id)  [21.8 ms]
                  ->  Hash Join (i.id = p.issue_id)
                        ->  Seq Scan on issues i  [17.5 ms, rows=50000]
                        ->  Bitmap Heap Scan on progress_records p  [rows=3255]
after   … Hash Join (s.id = started.series_id)
            ->  Seq Scan on series s  [0.3 ms, rows=2500]                       (info)
            ->  HashAggregate (i.series_id)  [6.0 ms]
                  ->  Hash Join (i.id = p.issue_id)
                        ->  Index Only Scan using issues_active_id_series_idx  [2.3 ms, heap fetches 0]
                        ->  Bitmap Heap Scan on progress_records p  [rows=3255]
```

With the default `random_page_cost = 4`, the planner prefers hashing the
whole active set over 3,255 primary-key probes. Probes would be about 5×
faster here, and they win outright under `random_page_cost = 1.1` (SSD).
The covering index makes the hashed side 5 MB instead of the 65 MB heap,
so the plan no longer depends on that tuning.

**recent-issues-rail**

```text
before  Limit [50.0 ms] -> Sort -> Subquery Scan on ranked [rows=7500]
          ->  WindowAgg (PARTITION BY series_id ORDER BY created_at DESC, id DESC; row_number() <= 3)
                ->  Sort [45.2 ms, 11.5 MB, rows=50000]
                      ->  Hash Join -> Seq Scan on issues i [rows=50000] + Seq Scan on series s
after   (per 128-row batch; two batches here)
        Limit [0.5 ms, rows=128]
          ->  Nested Loop
                ->  Index Scan using issues_active_created_idx on issues i
                      Index Cond: ROW(created_at, id) < ROW($ts, $id)       (batches ≥ 2)
                ->  Memoize -> Index Scan using series_pkey on series s
```

**series-search** (`q=crimson`, 50 matches), **series-recent**,
**series-filter-sort** and **saved-view-results**

```text
series-search before   Seq Scan on series [4.6 ms, removed 2,450]
                         Filter: search_doc @@ 'crimson' OR normalized_name % 'crimson'
series-search after    InitPlan: Unique(Append(Bitmap series_search_doc_gin [50], Bitmap series_normalized_name_trgm [50]))
                       ->  Index Scan using series_pkey (id = ANY(InitPlan))  [1.1 ms total]
series-recent before   Sort top-N ← Seq Scan on series [rows=2500]              [1.2 ms]
series-recent after    Index Scan using series_created_idx [rows=25]            [0.05 ms]
series-filter-sort     Hash Right Semi Join(series_genres via series_genres_genre_idx,
  before                 Seq Scan on series  Filter: year range AND publisher = 'Load Comics')
  after                  Bitmap Heap Scan on series ← Bitmap Index Scan on series_publisher_idx [311 rows])
saved-view-results     before: Seq Scan on series (publisher + year)            [0.56 ms]
                       after:  Bitmap Index Scan on series_publisher_idx → Bitmap Heap Scan [0.16 ms]
```

### Plans that were already index-driven

These plans are unchanged, apart from the projection shrinking row width.

```text
series-browse-name   Limit -> Incremental Sort (normalized_name, id)
                       -> Index Scan using series_library_normalized_uniq  Index Cond: library_id = $1   [0.5 ms]
series-browse-page2  same, + keyset Filter (normalized_name, id) > cursor                              [0.2 ms]
series-issues        Limit -> Sort (sort_number IS NULL, sort_number, id)
                       -> Index Scan using issues_series_slug_uniq  Index Cond: series_id = $1            [0.04 ms]
continue-reading     Nested Loop: Bitmap Index Scan progress_records_user_updated_idx (user_id)
                       → issues via pkey / issues_active_id_series_idx → series_pkey; top-N 24               [1.5–3 ms]
cbl-entries          Index Scan using cbl_entries_list_position_uniq (cbl_list_id)
                       + hydrate: Bitmap Index Scan on issues_pkey (id = ANY) [50 rows]                    [0.3 ms]
```

## M7 surfaces at 50,000 issues (WP-8.3)

The M7 / M7b relationship and similarity work (WP-7.1–7.8) was measured on
the 22k-issue dev library. This is the same set at the 50,000-issue stress
scale, from phase 2 of `just perf-explain`.

### Dataset

The stress fixture has credits, characters, genres and tags but none of
what the M7 surfaces read, so
[`seed_relationships.sql`](../../scripts/perf/seed_relationships.sql)
adds it deterministically on top of the scanned catalogue:

| what | shape | rows |
|---|---|---:|
| story arcs | 100, each named like its main series. Arc 1 is a mega-event: its main series plus one issue of every 6th series (~420 series). The other 99 have 4–17 tie-in series. | `issue_arcs` 3,127 · `series_arcs` 1,590 |
| universes | 8, holding 60 % of the series (~190 each) | `series_universes` 1,500 |
| series groups | every 5th series (groups of ~10; every 50th carries two) | 500 series |
| AlternateSeries | issue 1 of every 25th series names the next series | 100 issues |
| "large" series | every fixture writer + penciller credit (1,000) and 300 shared characters, in 40 arcs. This mirrors the dev library's Amazing Spider-Man 1989 (701 credits). | — |
| curated links | 50 six-series `sequel_of` chains, plus one hub series with 200 `see_also` links, as inverse pairs | 1,000 rows |
| from the job | the first run proposes 1,690 and writes the 1,000-row cap. 101 high-bucket rows are bulk-accepted, plus 400 more by explicit id (mega-event tie-ins first). | 1,401 edges · 1,000 pending · 501 accepted |

### Results

These figures come from a debug build with `auto_explain` per-node timing
on, on a shared workstation (load average 10–25 during the runs). As in
the top-ten table, treat them as relative.

**Endpoints** (final run, after the fixes below):

| endpoint | request | Σ SQL ms | slowest stmt ms | stmts | wall ms | gate |
|---|---|---:|---:|---:|---:|---|
| similar-large-cold | `GET /series/{large}/similar`, cache miss | 140.3 | 123.6 (overlap) | 11 | 454 | `series_tags` [expected] |
| similar-large-warm | same, neighbour cache hit | 1.7 | 0.7 | 8 | 47 | — |
| similar-typical-cold | `GET /series/{typical}/similar`, cache miss | 43.8 | 32.1 (overlap) | 11 | 116 | — |
| similar-typical-warm | same, cache hit | 4.0 | 1.8 | 8 | 53 | — |
| similar-home-rail | `GET /me/similar-series` | 14.8 | 13.4 (started series) | 8 | 44 | — (**was `issues`**) |
| relationships-chain | `GET /series/{s}/relationships`, middle of a 6-series chain | 0.6 | 0.2 | 12 | 44 | — |
| relationships-hub | same, 200 curated links | 9.8 | 4.5 (hydrate) | 12 | 40 | — |
| relationships-typical | same, no links | 0.3 | 0.0 | 11 | 44 | — |
| same-universe | `GET /series/{s}/same-universe` (195 series) | 5.3 | 1.7 | 9 | 34 | — (**was 29.0 ms Σ**) |
| same-universe-page2 | keyset page 2 | 5.4 | 3.3 | 8 | 42 | — |
| arc-tie-ins-mega | `GET /arcs/{mega}/tie-ins` (184 accepted tie-ins) | 18.6 | 10.6 (arc counts) | 11 | 91 | — |
| arc-tie-ins-mega-page2 | keyset page 2 | 8.1 | 3.6 | 10 | 38 | — |
| arc-tie-ins-typical | `GET /arcs/{arc}/tie-ins` | 1.8 | 0.9 | 11 | 42 | — |
| suggestions-pending | `GET /admin/relationship-suggestions?limit=50` (1,000 pending) | 3.9 | 1.3 | 10 | 39 | — |
| suggestions-page2 | keyset page 2 | 4.8 | 1.8 | 8 | 45 | — |
| suggestions-library | `?library_id=…&bucket=medium` | 5.7 | 2.0 | 10 | 33 | — |
| suggestions-all | `?status=all` | 3.8 | 1.0 | 10 | 25 | — |
| suggestions-series | `GET /series/{s}/relationship-suggestions` | 1.1 | 0.5 | 11 | 18 | — |

**The `relationship_suggest` job.** The first run proposes 1,690
candidates and writes 1,000 (the cap). The rerun follows 501 accepts.
`job elapsed` is the job's own `RunReport.elapsed_ms`; "wall" runs from
the `POST …/run` to the job's "run complete" log line.

| run | before fixes: job elapsed / wall ms | after: job elapsed / wall ms |
|---|---:|---:|
| first run (1,000 inserted) | 925 / 1,054 | 585 / 744 |
| rerun after accepts | 874 / 1,053 | 506 / 638 |
| with `auto_explain` on (plans) | 933 / 1,055 | 573 / 673 |

Per-source times (rerun, after) in ms:

- `name_continuation` 72, `provider_volume` 76, `translation` 71,
  `collected_edition` 37 (was 393), `arc_tie_in` 37, `alternate_series` 19.
- Every other source is ≤ 3.
- The remaining ~200 ms is merge, dedupe, the upserts and stale marking.

The job stays linear in the library: the per-series aggregates are
`CROSS JOIN LATERAL` index lookups over all 2,500 series. Its scans of
`issues` (the collected-edition and per-series reads) are whole-library
by design, and are reported `[batch]`.

**Bulk accept** (API, debug build): the high bucket (101 rows) took
133 ms; 400 explicit ids took 394 ms, about 1 ms per accept including the
inverse-pair write and audit row.

### Fixed in WP-8.3

1. **Home "similar series" rail: seq scan of `issues` on every load.**
   `started_series` joined `progress_records → issues` without the
   active-issue predicate, so the covering `issues_active_id_series_idx`
   could not serve it. The planner hashed a seq scan of the 65 MB heap,
   which was the only gate failure in the first run. The join now requires
   a live issue (`state = 'active' AND removed_at IS NULL`, the On Deck
   rule) and is an index-only scan. On the same database, measured with
   psql: 24–42 ms → 10–12 ms. The behaviour changes slightly: progress on a
   removed issue no longer marks its series "started".
2. **Same universe: JIT on a 1.6 ms query.** The planner estimates
   `regexp_split_to_table` at 1,000 rows per call. Over 500 grouped series
   that put both statements (count and page) past `jit_above_cost`, and
   JIT compilation took ~13–15 ms each. The equivalent
   `unnest(regexp_split_to_array(…))` is estimated at 10 rows and doesn't
   JIT. The count statement went from 27.0 ms to 1.8 ms (psql
   `EXPLAIN ANALYZE`), and Σ SQL for the request from 29.0 ms to 5.3 ms.
3. **Suggestion job, collected-edition source: regex per issue.** The
   series-level marker (series type, or a format word such as "TPB" or
   "Omnibus" in the name) sat inside an `OR` in the join filter. Postgres
   therefore evaluated the regex once per issue row (50,000 times) instead
   of once per series. A `MATERIALIZED` CTE now computes it per series.
   With psql the query went from 399 ms to 48 ms, and the job from about
   0.9 s to about 0.55 s.

No index was missing, so this WP adds no migration.

### Findings, not fixed (no redesign in this WP)

- **Similar series, cold, large series.** The overlap query is about
  125 ms for the 1,000-credit series (77,843 posting rows before the
  document-frequency filter; 55,396 after) and about 32 ms for a typical
  one. That is about 2× the dev library's 60 ms for 2.5× the overlap rows,
  so it is linear. Most of it is 1,000 `series_credits_role_person_idx`
  probes and the `live`/`df` aggregation. The df cut-off
  (`df ≤ n × 0.5`) applies only after every posting is collected. Pruning
  common entities before the postings join (for example a per-entity df
  table) would cut the large case, but it is a redesign of the scoring
  query. The in-process cache makes it a once-per-invalidation cost: warm
  requests take 1.7 ms. The `series_tags` seq scan is a fixture artifact:
  every stress series carries all eight tags.
- **JIT elsewhere in the job.** Two job statements still JIT, the
  name-continuation catalogue (~21 ms of JIT) and the detectors' catalogue
  (~13 ms), because the planner over-estimates them. This is harmless for
  a background job. The scanner's `story_arc` split
  (`metadata_rollup.rs`, `metadata/writers.rs`) and the AlternateSeries
  source use the same `regexp_split_to_table` shape and may JIT on larger
  libraries.
- **`series_universes` has no `universe_id`-leading index.** Same universe
  finds the other members through the `(series_id, universe_id)` primary
  key, which works on Postgres 18 thanks to skip scan ("Index Searches:
  1"). On Postgres 17 it would be a full index scan of a table that grows
  with series × universes. That is cheap at 1,500 rows, but worth an index
  if PG17 support matters.
- **Arc tie-ins** do the arc's series and issue counts as two
  `InitPlan`s per request (10.6 ms for the 420-series mega-event). This
  grows with arc size, not library size, so it is fine.
- **`GET /series/{slug}/relationships`** stays flat: 12 statements
  whether a series has 0, 2 (plus a 6-series chain) or 200 links.
  `perf_regressions.rs` now guards it with 500 links and a 13-node chain
  (≤ 15 queries, observed 12).
- **Harness bug, fixed:** `perf-explain.sh` killed only the subshell
  wrapping the server binary, so with `PERF_KEEP=1` the old server
  survived. On the next run it consumed jobs from the reused Redis. The
  binary is now `exec`ed.

## AR-1: the `issue::Entity::find()` audit

The audit counted about 80 sites that load full `issue::Model` rows. The
wide row (`comic_info_raw`, `pages`, ~20 CSV columns; 2.3 KB on average
here, more on real archives) is the most likely regression vector at scale.

The new
[`api::issue_card::IssueCardRow`](../../crates/server/src/api/issue_card.rs)
is a `DerivePartialModel` with the 14 columns an issue card, the ACL cap
check and the next-up walks read. `IssueSummaryView::from_model` now
delegates to `IssueCardRow::from(model).into_summary_view(..)`, so the
projected path and the full-row path share one mapping and cannot drift.
`access::filter_issues` and `access::issue_allowed` are generic over
`IssueAclFields`, which is implemented for both the full model and the
card row.

The partial model skips hydrating the JSON columns, but the heap tuples are
still read, so the gain is per-row transfer and deserialization rather than
I/O. The clearest example is `series-issues`, where the statement went from
0.98 ms to 0.06 ms for 20 rows. It matters most on paths that load every
issue of a series or list.

**Projected in this WP**

| path | handler | before |
|---|---|---|
| series issue grid (+ search mode) | `series::list_issues` | full rows, 50/page |
| series "Read" CTA | `series::resume` | **every issue in the series** |
| cross-library issues + search mode | `issues::list` | full rows, 50/page |
| issue search | `issues::search` | full rows |
| next / prev in series | `issues::next_in_series`, `prev_in_series` | full rows |
| continue-reading hydrate | `rails::continue_reading` | full rows ×24 |
| reader next-up / prev-up series walk | `next_up::pick_next_in_series_after`, `pick_prev_in_series_before` | **every issue in the series, on every issue open** |
| OPDS up-next (series) | `next_up::pick_next_in_series` | every issue in the series → now projected walk + 1 PK fetch of the pick |
| CBL next-up walk (reader + OPDS) | `next_up::scan_next_in_cbl` | **every matched issue of the list** → projected walk + 1 PK fetch |
| CBL entries / issues / window / window-paginated | `cbl_lists` (4 sites) | full rows; `/issues` = the whole list |
| collection entries + CBL export | `collections` (3 sites) | full rows; export = every issue of every series entry |
| mark series read / bulk progress | `progress` (3 sites) | every issue in the series |
| label / cover hydrates on list pages | `markers` (marker list), `reading_log` (series-cover pick + finished-issue index), `reading_sessions`, `admin_activity`, `scan_runs` (×2) | full rows for 2–4 columns |
| On Deck walks | `rails` (`OnDeckIssue` is now an alias of `IssueCardRow`) | already projected (WP-2.x) |

The count as of this WP covers 94 exact `issue::Entity::find()` sites
across `crates/server/src` (not `library_health_issue::Entity`):

- **59 projected.** They use `select_only`, `into_model`, `into_tuple` or
  the new `IssueCardRow`. The pre-existing ones are `series::hydrate_series`
  counts and covers, the `series::get_one` aggregates, `admin_thumbs` (6),
  `post_scan` (5), `reconcile`, `provider_ranges`, `libraries` scan preview,
  `account_export`, `slug`, `auto_split`, `orphan_sweep` and
  `reconcile_status`, plus `folder_checks`, which arrived with WP-3.4.
  `api/duplicates` (WP-3.3) uses raw projected SQL throughout.
- **11 `.count()`.** These never load rows.
- **24 full rows.**

**The 24 kept on full rows.** Each is a single-row or detail path, or a
path that genuinely reads the wide or many scalar columns:

- Issue detail: `issues::find_by_slugs` / `get_one`.
- Metadata editing and apply: `bulk_metadata`, `external_ids`,
  `metadata_search` (×4), `admin_metadata` and `metadata/apply`.
- Archive editing: `archive_edit`.
- The series detail hero-cover pick (`series::get_one`, one row).
- The reading-log event hydrate, which renders dates, credits and more per
  event.
- The scanner, deep validate, backfill and the post-scan worker's full
  issue fetch.
- The manual-writeback series fan-out (`metadata/manual_writeback`),
  which composes the sidecar XML from each full row.
- The OPDS feeds (see the backlog).

**Backlog: OPDS acquisition feeds** (`opds.rs` ×4, `opds_v2.rs` ×3). A
50-entry page hydrates full rows. The entries read about 25 scalar columns
(credits, summary, dates, size) but never `comic_info_raw` or `pages`. An
`OpdsEntryRow` projection that drops the two JSON blobs would help, but
`build_acquisition_feed` takes `&[issue::Model]` from about 20 call sites
across both OPDS versions, and no top-ten plan involves it, so it is left
as a follow-up.

## `oha` recipe (spec §18.3)

[`oha`](https://github.com/hatoo/oha) is a single-binary HTTP load
generator. Install it with `cargo install oha`, or let the script fall back
to the `ghcr.io/hatoo/oha` image. The quickest start is the built-in pass:

```sh
PERF_OHA=1 PERF_OHA_DURATION=30s PERF_OHA_CONCURRENCY=32 \
PERF_PG_CONTAINER=… PERF_REDIS_CONTAINER=… PERF_KEEP=1 just perf-explain
# → perf-out/explain-<ts>/oha-<label>.txt (latency histogram, RPS, status codes)
```

To drive a server by hand, whether the perf server, `just dev` or a
compose stack:

```sh
API=http://127.0.0.1:8080
# Log in once and reuse the session JWT as a Bearer token. Bearer requests skip CSRF.
curl -sS -c /tmp/folio.cookies -H 'Content-Type: application/json' \
  -d '{"email":"…","password":"…"}' "$API/auth/local/login" >/dev/null
TOKEN=$(awk '/comic_session/ {print $7}' /tmp/folio.cookies)
H=(-H "Authorization: Bearer $TOKEN")
```

| §18.3 scenario | recipe |
|---|---|
| 50 concurrent readers turning pages | `oha -z 60s -c 50 "${H[@]}" "$API/issues/$ISSUE_ID/pages/3"`. This hits one page, the cached path. To spread across pages, use `--rand-regex-url 'http://127\.0\.0\.1:8080/issues/<id>/pages/[0-9]'`, where the URL is a regex so dots must be escaped. Watch `folio_zip_lru_open_fds` on `/metrics`. |
| 1 active scan + 10 concurrent readers | start `curl -X POST "${H[@]}" "$API/api/libraries/$LIB_SLUG/scan?force=true"`, then immediately run the page-turn command with `-c 10 -z 120s`. Compare p99 against the idle run. |
| 1000-series library list pagination | `oha -z 30s -c 16 "${H[@]}" "$API/api/series?limit=50"`. For deep pages, take a `next_cursor` from one response and add `&cursor=…`. |
| Search query mix at 100 QPS | `oha -z 60s -q 100 -c 20 "${H[@]}" "$API/api/series?q=crimson&limit=8"` (autocomplete) alongside `oha -z 60s -q 20 "${H[@]}" "$API/api/issues/search?q=phoenix"` (full search). |

The recipes are about relative change: run before and after a change on
the same host. There are no committed pass/fail thresholds, because
absolute numbers depend on the hardware (the §18.1 budgets are for release
builds on production-like storage). The one hard invariant is the
`perf-explain` gate: no seq scan on an issue-scale table among these
endpoints. The k6 soak and Automerge sync-stress items in spec §16.5 are
not covered here.

## Observations from the stress scan

- About 100 files/s for 50,000 rich-metadata CBZs, debug build, thumbnails
  off, which matches [`scanner-perf.md`](scanner-perf.md).
- The scan logged 18 `duplicate key value violates unique constraint
  "person_slug_key"` errors. Concurrent workers insert the same new person
  (for example "Artist 068") at once. `ON CONFLICT (normalized_name)` does
  not cover the `slug` unique index, so the second insert fails with a
  unique violation. Every credit row still landed (verified with a
  writer/penciller-without-credit query), so this is log noise, not data
  loss. A conflict-free person upsert would be worth doing as a follow-up;
  it is not part of this WP.
