# OPDS catalog reference

Folio exposes its library to OPDS readers (Panels, Chunky, KOReader,
Calibre Companion, etc.) at two parallel surfaces:

- **OPDS 1.x (Atom XML)** under `/opds/v1/*` — the universally
  supported wire format. Every OPDS-capable client handles this.
- **OPDS 2.0 (JSON-LD)** under `/opds/v2/*` — the modern format with
  richer metadata, `groups[]` for multi-section payloads, and
  better facet support. Clients can negotiate via
  `Accept: application/opds+json`.

This page describes what each endpoint emits, what metadata
clients receive, and how the catalog hangs together. Auth and
ACLs (cookie session, app-passwords via HTTP Basic) are documented
separately in [opds-audit.md](opds-audit.md).

## Authentication

Three auth modes work:

1. **Cookie session** — browse from a logged-in browser (rare).
2. **HTTP Basic with app-password** — typical OPDS reader setup.
   Generate an app-password in `/settings/api-tokens`, point your
   client at `https://<host>/opds/v1` with `Basic <email>:<app-password>`.
3. **Bearer token** — for programmatic access. Same app-password
   sent as `Authorization: Bearer <token>`.

Unauthenticated requests return 401 with `WWW-Authenticate: Basic
realm="Folio"` so reader apps prompt for credentials.

## Endpoint catalog

### Core browsing

| Endpoint | Returns | Notes |
|---|---|---|
| `/opds/v1` | Root navigation feed | Subsection links to every other endpoint. |
| `/opds/v1/series` | Paginated series list | All series the user can see, alphabetical. 50/page. |
| `/opds/v1/series/{id}` | Per-series acquisition feed | Issues in the series. Banner metadata (cover + author + publisher + year + genres) at the feed root. |
| `/opds/v1/recent` | Newest 50 issues | Sorted by `created_at DESC`. |
| `/opds/v1/search?q=...` | Series matching `q` | Substring match against series name. |
| `/opds/v1/search.xml` | OpenSearch description | Lets clients auto-discover the search template. |

### Aggregation feeds (M5)

| Endpoint | Returns | Notes |
|---|---|---|
| `/opds/v1/continue` | Issues with progress in flight | Up to 24, newest-progress first. Same SQL as the web app's Continue Reading rail. |
| `/opds/v1/new-this-month` | Issues `created_at >= now - 30 days` | Up to 50. Goes empty on stale libraries (distinct from `/recent`). |
| `/opds/v1/by-creator/{writer}` | Series where any writer credit matches | URL-encode writer name. Empty result → empty feed (200 OK), not 404. |

### Faceted browse (M4)

| Endpoint | Returns | Notes |
|---|---|---|
| `/opds/v1/browse` | Series list with facet links | Stack `?status=...&publisher=...`. Active facets toggle off on re-click. |

Facet groups exposed:
- **Status** — `continuing`, `ended`, `hiatus`, `cancelled`.
- **Publisher** — top 20 publishers by series count.

### Personal surfaces

| Endpoint | Returns | Notes |
|---|---|---|
| `/opds/v1/wtr` | Want to Read shelf | Mixed series + issue entries. |
| `/opds/v1/lists` | The user's reading lists (CBLs) | Each CBL drills into its issue list. |
| `/opds/v1/lists/{cbl_id}` | One reading list | Issues in reading order. |
| `/opds/v1/collections` | The user's named collections | |
| `/opds/v1/collections/{id}` | One collection | Mixed series + issue entries. |
| `/opds/v1/views` | The user's saved filter views | |
| `/opds/v1/views/{view_id}` | One filter view's series | Same compile path as the web view. |
| `/opds/v1/pages` | The user's custom Pages | Drill-in to per-page feeds. |
| `/opds/v1/pages/{slug}` | One Page's pinned views | Per-kind dispatch: filter → /views, CBL → /lists, collection → /collections. |

### Acquisition + streaming

| Endpoint | Returns | Notes |
|---|---|---|
| `/opds/v1/issues/{id}/file` | Issue file bytes | Per-extension MIME (CBZ/CBR/PDF). Range / 206 supported. Audit-logged. |
| `/opds/pse/{id}/{n}?u=…&exp=…&sig=…` | One page image | HMAC-signed URL. Used by the OPDS Page-Streaming Extension (Chunky, Panels). |

### Progress sync (M7 of opds-readiness)

| Endpoint | Direction | Notes |
|---|---|---|
| `PUT /opds/v1/issues/{id}/progress` | Read → Folio | Per-issue `{page \| position, finished, device}`. Requires `read+progress` app-password scope. Full wire format in [opds-progress-protocol.md](opds-progress-protocol.md). |
| `PUT /opds/v1/syncs/progress/{hash}` | KOReader → Folio | KOReader Sync.app shim — accepts KOReader's wire format. |

### Sync surfaces (opds-sync-1.0)

| Endpoint / feature | Surface | Notes |
|---|---|---|
| `pse:last_read` + `pse:last_read_date` **attributes** on `<link rel=".../stream">` | per-issue PSE link (Atom v1) | Spec-conformant snake_case attributes per the [OPDS-PSE namespace](https://anansi-project.github.io/docs/opds-pse/specs). Drives "you're 14/32" badges in Panels / Chunky without a side-channel call. **Older docs showed `<pse:lastRead>` as a standalone camelCase element — that was a bug and is fixed in opds-sync-cleanup M1.** |
| `metadata.position` object | every issue publication (v2 JSON) | Readium-shaped `{position, totalProgression, modified, finished, totalPages}`. |
| `rel="next"` / `rel="previous"` | per-entry, series + CBL feeds | Sequential reading order. After finishing an issue, a client can follow `rel=next` to stream the next file without re-fetching the parent feed. |
| `rel="https://folio.local/rels/up-next"` | feed root, series + CBL | Folio-namespaced hint pointing at the issue the user should resume in *this* feed. Same resolver as the web On Deck rail. |
| Default up-next-first reorder | series / CBL / WTR / collection acquisition feeds (v1 + v2) | The next-unfinished issue is moved to entry index 0 on every reading-sequence feed. opds-sync-cleanup M2 made this the default for clients that ignore both `pse:last_read` and the up-next rel (most of them). The `?resume=1` synthetic-entry path is gone — reorder replaces it. |
| `preserve_canonical_order` opt-out | `series` / `cbl_lists` / `saved_views` rows + `users.opds_wtr_reorder` for WTR | Per-row escape hatch: setting the flag keeps canonical (sort_number / list position / drag-order) ordering even when up-next falls mid-list. Use for curated reading orders like "DC Year One". WTR is system-owned so its toggle lives on the user row instead. |
| Title-glyph + `(N / M)` suffix | every issue entry / publication (v1 + v2) | Each entry's title is decorated with a state glyph (`◯` unread / `◐` in progress / `●` finished) plus an `(N / M)` page suffix when total pages is known. Universal-compat cue for clients that ignore PSE attributes (Komga, KOReader, older Tachiyomi). Hide via `users.opds_progress_glyphs = false`. |
| `/opds/v1/continue` | aggregate "in-progress issues" | Newest-progress-first; carries feed-level up-next pointing at the most recent. |
| `/opds/v1/on-deck` | aggregate "what to read next" | One entry per active series / CBL the user is reading. Same data as the home OnDeck rail. |
| `/opds/v1/history` | finished issues, newest-first | Paginated. Powers "what did I read in March" queries. |
| `<pse:lastReadDate>` at CBL feed root | per-list "last read in this list" | MAX `progress_record.updated_at` across matched issues. |
| `numberOfRead` / `numberOfFinished` | v2 `/opds/v2/lists` nav entries + CBL acq metadata | "5 of 24 finished" inline on the list summary. |
| Implicit progress on PSE stream hits | server-side, transparent to client | Every `GET /opds/pse/{id}/{n}` fires a fire-and-forget progress upsert. Monotonic-only — buffered prefetches behind current position are dropped. `finished=true` set on last-page hit. Device tag `"opds-pse"`. |
| `rel="http://opds-spec.org/sync"` | catalog root (v1 + v2) | Discoverable write-back endpoint advertisement. v2 carries the `profile` URL anchoring the [protocol doc](opds-progress-protocol.md). |

### Client compatibility matrix

Verified behavior is marked **(verified)**. Other entries reflect
each client's documented capabilities; we haven't yet driven a real
build end-to-end against Folio's full sync surface — when you exercise
one, please update this table.

| Client | Read OPDS v1 | Read OPDS v2 | Write progress | Honors `rel=next` | Honors `pse:last_read` | App-password required |
| --- | --- | --- | --- | --- | --- | --- |
| Panels (iOS) | ✓ | partial | implicit (PSE) | manual UI | ✓ | yes — `read+progress` scope |
| Chunky (iOS) | ✓ | no | implicit (PSE) | manual UI | ✓ | yes — `read+progress` scope |
| KOReader | ✓ | no | explicit (sync.app) | manual UI | ✓ | yes (Basic auth carries app-password) |
| Calibre Companion | ✓ | no | none | no | no | yes — read scope sufficient |

**Per-client gotchas:**

- **Panels** — Honors the PSE stream and writes implicit progress
  via that path. The explicit `PUT /opds/v1/issues/{id}/progress`
  endpoint isn't called; rely on the implicit signal. The
  M5 `<uri>` drill-in on author chips works.
- **Chunky** — Same as Panels for progress. Less complete metadata
  rendering (limited `<category>` support). Requires the
  per-extension MIME on the acquisition link.
- **KOReader** — Calls the `/opds/v1/syncs/progress/{hash}` shim,
  not the explicit per-issue endpoint. Uses percentage (0.0-1.0),
  which Folio converts to integer page. Document-hash is BLAKE3 of
  the file bytes — Folio uses the same hash as `issue.id`, so the
  shim is a thin format adapter.
- **Calibre Companion** — Read-only by design. No progress write
  back; treat it as a discovery client.

**Capability auto-detection.** Capable clients can detect Folio's
write-back support via the catalog-root rel:

```xml
<link rel="http://opds-spec.org/sync"
      href="/opds/v1/issues/{issue_id}/progress"
      type="application/json"/>
```

The rel is Folio-namespaced (no canonical OPDS sync rel exists yet).
The OPDS 2.0 mirror carries the same href plus a `profile` URL
(`https://folio.bryhome.live/spec/progress-write-v1`) anchoring the
documented wire format — see [opds-progress-protocol.md](opds-progress-protocol.md).

## OPDS 2.0 (JSON) at a glance

Every `/opds/v1/*` endpoint has an `/opds/v2/*` mirror with
identical data and ACLs, but JSON instead of Atom. Content
negotiation: send `Accept: application/opds+json` to the v1
endpoint and Folio 308-redirects to the v2 equivalent.

Two v2-only features:

1. **`/opds/v2` root carries `groups[]`** — inlined previews of
   Continue reading + New this month + All series in one payload,
   so capable clients render a multi-section home in a single
   round-trip.
2. **`/opds/v2/browse` exposes `facets[]`** as a top-level array
   instead of inline `<link rel=facet>` elements. Each facet link
   carries `properties.numberOfItems` (the count under that facet)
   and `properties.active` (the current selection).

## Per-entry metadata

Every series entry across all browse surfaces carries:

| Atom (v1) | JSON (v2) | Source |
|---|---|---|
| `<title>` | `metadata.title` | `series.name` |
| `<link rel=image/thumbnail>` | `images[]` (thumbnail) | First active issue's page 0 |
| `<link rel=image>` | `images[]` (full) | Same, full-size |
| `<dc:publisher>` | `metadata.publisher.name` | `series.publisher` |
| `<dc:issued>` | `metadata.published` | `series.year` |
| `<dc:language>` | `metadata.language` | `series.language_code` |
| `<author><name>...</name><uri>...</uri></author>` | `metadata.author[]` | `series_credits` where `role=writer` |
| `<category term=... scheme="urn:folio:genre">` | `metadata.subject[]` | `series_genres` |
| `<content type=html>` or `<summary>` | `metadata.description` | `series.summary` |
| `<link rel=subsection>` | `href` (drill-in) | `/opds/v1/series/{id}` |
| `<link rel=alternate type=application/json>` | `links[]` (alternate) | `/api/series/{slug}` — canonical JSON |

The `<uri>` element inside `<author>` is the M5 drill-in: clicking
the author name in an OPDS reader navigates to
`/opds/v1/by-creator/{name}` — every series by that writer.

## Renderer notes (Panels / Chunky / KOReader)

The OPDS spec doesn't mandate how clients render any of this — each
app makes its own UI decisions. Empirically:

- **Panels** renders cover art prominently in browse, byline +
  publisher + year + genre chips on the series detail screen.
  Honors the M5 `<uri>` drill-in. **Progress resume:** Folio emits
  PSE progress hints on both the stream link AND the regular
  acquisition link, in both snake_case (`pse:last_read` /
  `pse:last_read_date`) and camelCase (`pse:lastRead` /
  `pse:lastReadDate`) spellings, with **1-indexed display position**
  as the value (`progress_record.last_page + 1`). The dual emission
  exists because the Anansi PSE spec is ambiguous on case while
  Komga/Kavita and Panels read camelCase, and the dual-link emission
  covers both page-streaming and full-download flows. Without this
  Panels falls back to the cover (display page 1) — see the
  regression in `opds_richer_feeds_1_1::pse_progress_attrs_emit_1_indexed_and_both_spellings`.
- **Chunky** renders cover + title + author inline in the browse
  grid. Has limited support for `<category>` chips.
- **KOReader** renders a list view (no grid). Shows author + year
  inline, genres as a comma-separated list.
- **Calibre Companion** focuses on issue-level metadata; series
  metadata is treated as folder labels.

When in doubt about what an OPDS surface looks like in a specific
client: tap a series cover. Most clients show metadata only in the
detail view, not in the browse grid.

## Self-host configuration

The OPDS surface needs no specific config — it runs out of the
box. Two practical recommendations:

- **Use a dedicated app-password per device.** Revoke from
  `/settings/api-tokens` if a reader is lost.
- **Cloudflare** in front of Folio: don't enable Scrape Shield's
  "Email Address Obfuscation" toggle. Its injected script trips
  the strict CSP and breaks the web UI; OPDS is unaffected but
  the dependency on a working web app is real. See
  [cloudflare.md](../install/cloudflare.md).

## Plan history

The OPDS surface evolved through four completed plans:

- `~/.claude/plans/done/opds-readiness-1.0.md` — Phase-2 OPDS readiness
  (M1-M7). Established the baseline catalog, page streaming
  extension, progress sync, KOReader shim, OPDS 2.0 JSON mirror.
- `~/.claude/plans/done/opds-richer-feeds-1.0.md` — Visual + UX polish
  (M1-M7). Added cover art on series entries, rich metadata, custom
  Pages, faceted browse, aggregation feeds, OPDS 2.0 groups[], and
  `rel=alternate` JSON links.
- `~/.claude/plans/done/opds-sync-1.0.md` — End-to-end progress sync.
  Inline read-state annotations (M1), sequential rel=next/previous
  (M2), feed-level up-next rel (M2.3), `/continue` + `/on-deck`
  aggregate feeds (M2.5), implicit PSE progress writes (M3),
  write-back advertisement (M4), history feed + conflict-resolution
  tests (M5), bidirectional CBL progress (M6). Sync details in
  [opds-progress-protocol.md](opds-progress-protocol.md).
- `~/.claude/plans/done/opds-sync-cleanup-1.0.md` — Spec + UX-fit
  cleanup. M1 moved `pse:last_read` / `pse:last_read_date` to
  snake_case attributes on the PSE stream link (the original
  `<pse:lastRead>` element shape didn't match the spec and Panels
  ignored it). M2 replaced the opt-in `?resume=1` synthetic entry
  with a default up-next-first reorder across every reading-sequence
  feed (series + CBL + WTR + collections), with per-row
  `preserve_canonical_order` opt-outs. M3 added a title-level glyph +
  `(N / M)` suffix for universal-compat progress visibility on
  clients that ignore PSE entirely.
- `~/.claude/plans/done/opds-richer-feeds-1.1.md` — three follow-ups
  triggered by Panels feedback. M1 prefixes the up-next entry's
  title with `"Up Next: "` so clients that don't honor the feed-level
  rel still surface the resume target. M2 mirrors `pse:last_read` /
  `pse:last_read_date` onto the regular acquisition link (Panels
  reads them there on first download). M3 reorganizes the root
  navigation around `user_page`: Continue reading + On Deck stay
  top-level, every user-page becomes its own folder, and the
  redundant `All series` / `Recently added` / `Read history` /
  `New this month` / `Want to Read` / `My pages` shortcuts are
  removed (their handlers stay URL-addressable; pin to a page or
  use the existing catch-all feeds to reach them). `Browse` becomes
  the canonical entry point to the whole library (it was a superset
  of `All series` — facets added).

Regression guards across 15 test files: `opds_inline_progress`,
`opds_sequential_nav`, `opds_up_next_rel`, `opds_default_reorder`,
`opds_progress_glyphs`, `opds_personal_feeds`,
`opds_pse_implicit_progress`, `opds_progress_advertisement`,
`opds_history_and_conflicts`, `opds_cbl_progress`,
`opds_richer_feeds_1_1`, plus the v2 mirrors.
(`opds_resume_synthetic.rs` was deleted in cleanup M2 — the path
it exercised no longer exists.)

All five plans are complete. Future OPDS work would target the
deferred items in those plans (reading-status facet, empty-series
placeholder thumbnails, OpenSearch facet params, Automerge CRDT
progress merge) or new surfaces discovered through user feedback.
