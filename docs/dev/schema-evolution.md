# Schema evolution

Folio's Postgres schema has gone through three distinct eras. This doc names
them, links the landmark migrations, explains a few intentional
gaps between the `entity/` crate and the live DB, and records the
index-coverage audit that closed audit-remediation M8.4.

## Eras

### Era 1 — foundation (2026-01 → 2026-04)

The initial schema lived in `m20260101_*` (users, auth_sessions,
audit_log, library_user_access) and `m20260201_*` (libraries, series,
issues, progress placeholder). Everything was normalised; constraints
were minimal. Search support came in [m20260301_000001_search_docs](../../crates/migration/src/m20260301_000001_search_docs.rs)
with `tsvector` GENERATED ALWAYS columns + GIN indexes — the only
generated columns we ship.

### Era 2 — denormalisation for application shape (2026-05 → 2026-09)

As the product surface grew, the schema picked up application-shaped
columns:

- **[m20260507_000001_add_slugs](../../crates/migration/src/m20260507_000001_add_slugs.rs)** — slug allocator + slug columns on libraries/series/issues. Replaced the locale-prefixed URLs of v0.1.
- **[m20260801_000001_scanner_v1](../../crates/migration/src/m20260801_000001_scanner_v1.rs)** — soft-delete columns (`removed_at`, `removal_confirmed_at`, `superseded_by`), scan_runs, library_health_issues. Soft-delete was chosen so the scanner could surface "this file moved" without losing the user's progress row.
- **[m20260902_000001_thumbnail_state](../../crates/migration/src/m20260902_000001_thumbnail_state.rs)** — thumbnail status JSONB on issues. Picked JSONB over four boolean/text columns so the variant set (cover / strip / pages) could grow without a migration. Cost: querying state requires JSONB ops.

The library scanner's [docs/dev/library-scanner.md](library-scanner.md) covers
the why behind soft-delete + content-hash decoupling, both shaped in Era 2.

### Era 3 — re-normalisation for filters + identity (2026-10 → 2026-12)

The saved-smart-views plan + creator-pages plan drove re-normalisation:

- **[m20261203_000001_metadata_junctions](../../crates/migration/src/m20261203_000001_metadata_junctions.rs)** — replaced CSV-shaped `series.genre` / `series.tags` columns with junction tables (`series_genres`, `series_tags`, `series_credits`, mirrored at issue level). Filter views can now index against `(genre, series_id)` instead of LIKE-scanning a CSV. The eight per-role credit columns (`writer`, `penciller`, …) collapsed into a single `(role, person)` shape.
- **[m20261219_000001_character_team_location_junctions](../../crates/migration/src/m20261219_000001_character_team_location_junctions.rs)** — same re-normalisation pass for the character/team/location facets that ship in `comicinfo.xml`.
- **[m20261223_000001_person](../../crates/migration/src/m20261223_000001_person.rs)** — person entity with stable slugs, derived from the `person TEXT` column on credits. The text column stays as the denormalised source of truth so filter joins don't gain a hop; the person table is the URL-routable index.

## Entity ↔ DB parity rule

Every Postgres column should be reachable through an entity in
`crates/entity/`, with two narrow exceptions:

1. **Postgres `GENERATED ALWAYS` columns** — the `search_doc` columns on `series` and `issues`. Sea-ORM has no first-class support for read-only generated columns; including them would require writing custom `ActiveModel` plumbing that rejects writes. Cleaner to leave them off the entity and let the column live only in the migration. The columns are explicitly documented in the entity files (look for the `search_doc is a Postgres GENERATED ALWAYS column` doc comment in [series.rs](../../crates/entity/src/series.rs) and [issue.rs](../../crates/entity/src/issue.rs)).
2. **SQL views** — `user_series_progress` ([m20261204_000001_user_series_progress_view](../../crates/migration/src/m20261204_000001_user_series_progress_view.rs)) is a view, not a table. Sea-ORM treats views just like tables for `SELECT` (we have an entity at [user_series_progress.rs](../../crates/entity/src/user_series_progress.rs)) but a parity test that walks `information_schema.tables` should skip it; the corresponding row lives in `information_schema.views`.

The parity rule is enforced by [crates/server/tests/schema_parity.rs](../../crates/server/tests/schema_parity.rs)
(audit-remediation M8.3). A new column without an entity update fails the
test; the same allow-list above is the only escape hatch.

## Index audit (M8.4)

Audit-remediation M8.4 catalogued the indexes on the hot read paths flagged
in the audit. Findings:

| Read path                                | Existing index                                            | Source                                                                                                          | Adequate? |
|------------------------------------------|-----------------------------------------------------------|-----------------------------------------------------------------------------------------------------------------|-----------|
| `/admin/audit?actor_id=…`                | `audit_log_actor_idx (actor_id, created_at)`              | [m20260101_000005_audit_log](../../crates/migration/src/m20260101_000005_audit_log.rs)                          | ✓         |
| `/admin/audit?target_id=…`               | `audit_log_target_idx (target_id, created_at)`            | same                                                                                                            | ✓         |
| `/admin/audit?action=…`                  | `audit_log_action_idx (action, created_at)`               | same                                                                                                            | ✓         |
| `/me/reading-log?from=…&to=…`            | `progress_records_user_updated_idx (user_id, updated_at)` | [m20260201_000003_progress_placeholder](../../crates/migration/src/m20260201_000003_progress_placeholder.rs)    | ✓         |
| `/me/reading-log?finished=true`          | `progress_records_user_finished_at (user_id, finished_at)`| [m20260522_000001_progress_records_finished_at](../../crates/migration/src/m20260522_000001_progress_records_finished_at.rs) | ✓         |
| `/me/markers` (no kind filter)           | `markers_user_kind_updated_idx (user_id, kind, updated_at DESC)` | [m20261215_000002_markers](../../crates/migration/src/m20261215_000002_markers.rs)                              | ✓ leading prefix on `user_id`; extra in-memory sort across `kind` is acceptable at the per-user marker counts we expect (10s–100s) |
| `/me/markers?issue_id=…` (reader overlay)| `markers_user_issue_page_idx (user_id, issue_id, page_index)`   | same                                                                                                            | ✓         |
| `/me/sessions` (per-user listing)        | `reading_sessions_user_started_idx (user_id, started_at DESC)` | [m20261001_000001_reading_sessions](../../crates/migration/src/m20261001_000001_reading_sessions.rs)            | ✓         |
| `/admin/stats/users` (paginated)         | primary key on `users.id` (cursor key)                    | [m20260101_000002_users](../../crates/migration/src/m20260101_000002_users.rs)                                  | ✓         |

**No new index migrations needed for M8.4.** Every read path the audit
flagged is already covered by an existing index — the plan's "pending
evidence" stance was correct, and the evidence (a hand-walk of the
migration set against the query shapes in `crates/server/src/api/`)
came back clean.

Future index decisions should be evidence-driven through the
deferred query-count regression suite (audit-remediation M5.5 →
landing in M10) — adding indexes pre-emptively only buys storage and
write-amp.
