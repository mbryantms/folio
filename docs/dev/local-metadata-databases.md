# Local metadata databases (GCD + Local ComicVine)

Operator-supplied SQLite copies of a provider's catalogue that Folio
reads instead of, or ahead of, the provider's API. Plan and decisions:
`~/.claude/plans/local-metadata-databases-1.0.md` (review doc linked
there). This page tracks what has shipped.

## The files

| | Local GCD | Local ComicVine |
| --- | --- | --- |
| Where it comes from | comics.org › Download › "Private Use" › SQLite (free account, CC BY-SA 4.0, attribution required) | A community-built SQLite copy of ComicVine's catalogue. Folio reads one if it is there; it never downloads, links to or updates it |
| Required tables | `gcd_series`, `gcd_issue`, `gcd_story`, `gcd_publisher` | `cv_volume`, `cv_issue`, `cv_publisher` |
| Optional tables | `gcd_story_credit`, `gcd_issue_credit`, `gcd_creator`, `gcd_language`, `gcd_indicia_publisher`, `gcd_series_bond` | `cv_person`, `cv_sync_metadata`, `volume_fts`, `comic_covers` |
| Dump date | newest `gcd_issue.modified` | newest `cv_sync_metadata.last_sync_date` |

Put the file on the server's own disk (not a network share) and mount
it into the container; Folio opens it **read-only and immutable**, so
it never takes a lock and never writes to it. Replace a file by renaming
a new one into place; the next open sees it.

## Settings (M1)

| Key | Values | Config field |
| --- | --- | --- |
| `metadata.gcd.local_path` | absolute path | `gcd_local_path` |
| `metadata.gcd.local_mode` | `off` (default) · `prefer_local` · `local_only` | `gcd_local_mode` |
| `metadata.comicvine.local_path` | absolute path | `comicvine_local_path` |
| `metadata.comicvine.local_mode` | as above | `comicvine_local_mode` |

A mode string the enum does not know logs a warning and degrades to
`off`; the file is never read on a typo. Modes take effect once the
backends land (M2 GCD, M3 ComicVine); in M1 they are stored and shown.
Per-library overrides arrive with M2.

- `off` — the API only (today's behaviour).
- `prefer_local` — the file answers first; the API is asked only for
  what the file has no record of. Never by date (decision 7).
- `local_only` — the file only; a missing record is `NotFound`.

## Admin surface (M1)

Each ComicVine and GCD card under Admin › Metadata › Providers carries
a **Local file** block: path, mode, status (`unconfigured` · `ok` ·
`missing` · `invalid`), dump date with a stale warning past 60 days,
row counts, missing optional tables, and a **Test file** button.
`POST /admin/metadata/providers/{id}/local/test` re-probes past the
cache and is audited as `admin.metadata.local_db.test`. The list view
caches a probe for 10 minutes in Redis, keyed by the file's size and
mtime, so counting a 2 M-row table does not run on every page load.

Code: `crates/server/src/metadata/local_db.rs` (modes, open, probe),
`api/admin_metadata.rs` (`LocalDbView`, `test_local_db`),
`web/components/admin/metadata/ProviderConfigForm.tsx` and
`ProvidersTab.tsx`. Tests: `crates/server/tests/local_metadata_db.rs`
(fixture files built in the dumps' shapes; set `FOLIO_LOCAL_CV_DB` /
`FOLIO_LOCAL_GCD_DB` and run the ignored test to probe real files).

## Attribution

Pages that render GCD data carry a footer line crediting the Grand
Comics Database with a link (decision 8). The ComicVine copy is always
labelled "Local ComicVine".
