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
| `PUT /opds/v1/issues/{id}/progress` | Read → Folio | Per-issue `{page, finished}`. Requires `read+progress` app-password scope. |
| `PUT /opds/v1/syncs/progress/{hash}` | KOReader → Folio | KOReader Sync.app shim — accepts KOReader's wire format. |

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
  Honors the M5 `<uri>` drill-in.
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

The OPDS surface evolved through two completed plans:

- `~/.claude/plans/opds-readiness-1.0.md` — Phase-2 OPDS readiness
  (M1-M7). Established the baseline catalog, page streaming
  extension, progress sync, KOReader shim, OPDS 2.0 JSON mirror.
- `~/.claude/plans/opds-richer-feeds-1.0.md` — Visual + UX polish
  (M1-M7). Added cover art on series entries, rich metadata, custom
  Pages, faceted browse, aggregation feeds, OPDS 2.0 groups[], and
  `rel=alternate` JSON links.

Both plans are complete. Future OPDS work would target the
deferred items in those plans (reading-status facet, empty-series
placeholder thumbnails, OpenSearch facet params) or new surfaces
discovered through user feedback.
