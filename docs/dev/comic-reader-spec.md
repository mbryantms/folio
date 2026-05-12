# Comic Reader — Application Specification

> **Status:** Draft v0.5 — living document. Iterate freely.
> **Purpose:** Single source of truth for building a self-hostable, multi-platform comic reader. Written to be terse and unambiguous so it can be fed directly to Claude Code.
> **Build priority:** Server + web first. iOS later. Android maybe never. See §1.1.

---

## Table of Contents

1. [Product Summary](#1-product-summary)
2. [Architecture](#2-architecture)
3. [Tech Stack](#3-tech-stack)
4. [File & Format Support](#4-file--format-support)
5. [Library Model](#5-library-model)
6. [Search](#6-search)
7. [Reader](#7-reader)
8. [OPDS](#8-opds)
9. [Sync](#9-sync)
10. [API Surface (sketch)](#10-api-surface-sketch)
11. [Mobile Apps (deferred)](#11-mobile-apps-deferred)
12. [Self-Host & Ops](#12-self-host--ops)
13. [Local Development Environment](#13-local-development-environment)
14. [Repository Layout](#14-repository-layout)
15. [Conventions](#15-conventions)
16. [Testing Strategy](#16-testing-strategy)
17. [Security](#17-security)
18. [Performance Targets](#18-performance-targets)
19. [Roadmap](#19-roadmap)
20. [Decisions & Open Questions](#20-decisions--open-questions)
21. [Backlog (deferred but tracked)](#21-backlog-deferred-but-tracked)
22. [Glossary](#22-glossary)

---

## 1. Product Summary

A self-hostable comic reading platform. Reads `.cbz` (primary), `.cbr`, `.cb7`, and folder-of-images. Parses `ComicInfo.xml`, `series.json`, and `MetronInfo.xml`. Surfaces all metadata. Syncs reading progress across devices. Serves OPDS.

### 1.1 Build priority (firm ordering)
1. **Server (Rust/Axum)** — first-class, must be solid before anything else.
2. **Web app (Next.js)** — first-class, primary client; ships alongside the server.
3. **iOS app (SwiftUI)** — secondary, built only after server + web are stable and feature-complete through Phase 6.
4. **Android app (Kotlin/Compose)** — tertiary, may never be built. Treat as aspirational.

The server's API and OpenAPI contract are designed to support all four clients regardless of build order. Mobile-specific design choices (e.g., shared core, Automerge bindings) are flagged in-spec but not implemented in the early phases.

### 1.2 Non-goals (v1)
- Library management/tagging UI on par with Mylar3.
- File editing, repacking, or any modification of user `.cbz` files.
- Social features (community reviews, public profiles, sharing across servers).
- Transcoding to other formats (with one exception: server-side JXL → AVIF/WebP transcode for browser delivery — see §7.2).
- In-image translation (deferred indefinitely; see §21 Backlog).
- **PDF input.** `.pdf` files are explicitly out of scope; the scanner ignores them. Rationale: PDF rendering requires a heavyweight native dependency (`pdfium`/`mupdf`) with its own CVE history, and PDF comics are best converted to CBZ by the user with a one-shot tool. Revisit if and only if there is sustained user demand and a sandbox-able decoder lands.
- **Encrypted/password-protected archives.** Detected at scan time and surfaced in the admin UI as `issue.encrypted`; never prompted for. See §4.6.

### 1.3 Primary user
Self-hosters running the server in Docker behind a reverse proxy with SSO. Single-tenant or small-family multi-tenant deployments. Not designed for large public hosting.

---

## 2. Architecture

```
┌──────────────────────┐  ┌──────────────────┐  ┌────────────────────┐
│ Web App (Next.js)    │  │ iOS (later)      │  │ Android (maybe)    │
│ — primary client —   │  │ — secondary —    │  │ — tertiary —       │
└──────────┬───────────┘  └────────┬─────────┘  └─────────┬──────────┘
           │                       │                      │
           └───────────┬───────────┴──────────┬───────────┘
                       │     HTTPS / WSS      │
                       ▼                      ▼
              ┌──────────────────────────────────────┐
              │   API Gateway (Rust / Axum)          │
              │   • REST + OPDS + WebSocket          │
              │   • OIDC (Authentik) + JWT           │
              └────┬───────────────────┬─────────────┘
                   │                   │
          ┌────────▼──────┐   ┌────────▼────────┐
          │  Library      │   │  Sync/Progress  │
          │  Service      │   │  Service        │
          │  (scanner,    │   │  (Automerge,    │
          │   metadata,   │   │   WebSocket)    │
          │   thumbs)     │   │                 │
          └────────┬──────┘   └────────┬────────┘
                   │                   │
                   ▼                   ▼
          ┌────────────────────────────────────────┐
          │ Postgres │ Redis (cache/queue) │ FS    │
          │   (extensions: pg_trgm, unaccent,      │
          │    fuzzystrmatch)                       │
          └────────────────────────────────────────┘
```

Note: "Library Service" and "Sync/Progress Service" are *logical* modules within the single Axum binary, not separate processes. Splitting into separate services is a future option, not a v1 goal. The container also runs Next.js standalone as a sibling process under `tini` (§12.1.2) — this is a packaging detail, not an architectural service.

### 2.1 Module responsibilities (within the single server binary)
- **API module** — auth, routing, OPDS feed assembly, streaming page bytes, rate limiting, OpenAPI spec generation.
- **Library module** — recursive scan, archive inspection, metadata parse, thumbnail generation (WebP), cover extraction, hash-based dedupe, file watch (`notify` crate).
- **Sync module** — per-user Automerge documents (progress, collections, reader settings); WebSocket push.
- **Search module** — query construction across `tsvector` indexes, ranking, "did you mean" generation.

### 2.2 Storage
- **Postgres** — all structured data: metadata, users, progress (Automerge BYTEA), reviews, collections, search indexes.
- **Redis** — session cache, scan queue, pub/sub for sync fanout. Optional in dev (in-process fallback).
- **Filesystem** — original `.cbz` files (read-only mount); generated thumbnails in a writable `data/` directory.

No object storage in v1 (no S3/MinIO). Filesystem is sufficient and simpler to back up.

---

## 3. Tech Stack

### 3.1 Server & Web (priority — v1)

| Layer | Choice | Why |
|---|---|---|
| Server | Rust + Axum + Tokio | Streaming page bytes, low memory under load, single static binary |
| DB ORM | SeaORM + sea-orm-migration | Relationship-heavy schema (series relationships, reading lists, collections); typed entities with `Linked` trait for self-referential edges. Drop to `sea_query` or raw SQL via the connection for hot-path reads. **Do not** mix sqlx-cli — use SeaORM's migrator exclusively. |
| Web | Next.js 15 (App Router) + React 19 + TypeScript | RSC for data-heavy pages, mature ecosystem, full library coverage. Self-hosted via `output: 'standalone'` in distroless container — **not** Vercel-deployed. |
| Web UI | shadcn/ui (Radix primitives) + Tailwind v4 | Headless, accessible, copy-in components. Best-in-class for accessibility and customization. |
| Web state/data | TanStack Query v5 + Zustand | Query for server state (caching, optimistic updates, sync); Zustand for reader-local state (zoom, page, settings). |
| Web forms | React Hook Form + Zod | Type-safe schemas shared with API client. |
| Web reader libs | `@use-gesture/react` (reader gestures) `[reader-route: yes]`, `@tanstack/virtual` (huge libraries) `[reader-route: no]`, `dnd-kit` (collection reorder) `[reader-route: no]`, Tiptap (review editor) `[reader-route: no]`, `framer-motion` (page transitions in non-reader views) `[reader-route: no]` | Each best-in-class; all React-first. Lazy-loaded per route. **The reader route uses CSS transitions for page-turn animation** — `framer-motion` is excluded from the `/read/[issue]` bundle (§18.1 budget). |
| Web image pipeline | OffscreenCanvas + `createImageBitmap()` in Web Workers; AVIF/WebP/JPEG/PNG/GIF decoded by browser; **JXL transcoded to AVIF server-side** | Modern decode off the main thread, low CPU. WebCodecs `ImageDecoder` is *not* used — `createImageBitmap(blob, …)` is sufficient for static images and has broader format support. JXL has no reliable browser decoder as of 2026, so the server transcodes; the original is preserved for OPDS/download. |
| API client (web) | `openapi-typescript` + `openapi-fetch` | Generates types from OpenAPI spec, ~6KB runtime, fully typed. Wrap with TanStack Query manually. |
| Sync (CRDT) | Automerge 2.x (Rust core, JS/Swift/Kotlin bindings) | Cross-platform, mature, future-proofs collaborative features (annotations, shared lists). Used for progress sync, bookmarks, ratings. |
| Shared API contract | OpenAPI 3.1, generated from Rust source via `utoipa` | One source of truth. Web client generated from this. Mobile clients will consume the same spec. |
| Realtime | WebSocket + Server-Sent Events fallback | Progress sync, scan events |
| Auth | `COMIC_AUTH_MODE=oidc\|local\|both`. OIDC (Authentik / Keycloak / Dex) → JWT primary; local users (argon2id + optional TOTP) for installs without an OIDC IdP | Matches existing homelab; local mode broadens the self-host audience without making OIDC mandatory. See §17.2. |
| Web session | httpOnly + `SameSite=Lax` + `Secure` cookie holding signed JWT; double-submit `X-CSRF-Token` on unsafe verbs; one-shot WebSocket auth ticket | Closes the CSRF surface that pure-Bearer-in-localStorage opens. See §17.3. |
| Background jobs | `apalis` (Redis-backed) for scan, thumbnail generation, suggestion engine, search-dictionary build | Mature, retries, scheduling. Makes Redis effectively required in prod; in-process fallback only for `just dev`. |
| Container | Distroless Docker, multi-arch (amd64/arm64); base image includes `unrar` for `.cbr` support | Small attack surface |

### 3.2 Mobile (deferred — design only, no implementation in early phases)

| Layer | Choice | Why |
|---|---|---|
| iOS | Swift 6 + SwiftUI + SwiftData | Native, structured concurrency, offline-first |
| Android | Kotlin Multiplatform + Compose | Share core models with iOS later if desired |

These rows document the *intended* mobile stack so server/API decisions stay compatible. They are **not** implemented until Phase 7+.

---

## 4. File & Format Support

### 4.1 Archives
- **Primary:** `.cbz` (ZIP). Stored uncompressed reads via direct seek.
- **Secondary:** `.cbr` (RAR — read-only via host-installed `unrar` binary; provided by the Docker base image; license note in `LICENSE-THIRD-PARTY.md`), `.cb7` (7z via `sevenz-rust`), `.cbt` (tar, stdlib), folder-of-images, `.epub` (comic-style only — predicate in §4.5).
- **Image formats inside (allowlist):** JPEG, PNG, WebP, AVIF, JXL, GIF. Anything else is skipped at scan time and surfaced as a per-issue warning. **SVG entries are rejected outright** (script-bearing).
- **Page ordering:** natural sort (numeric-aware), then case-insensitive lex. Skip files matching `^\.|^__MACOSX|Thumbs\.db|\.xml$|\.json$|\.txt$`.

### 4.1.1 Archive defense limits (apply to every archive type)
Every archive parser enforces these caps before extraction; violation = typed error, archive marked `issue.malformed` and excluded from the library:

| Limit | Default | Env override |
|---|---|---|
| Max entries per archive | 50 000 | `COMIC_ARCHIVE_MAX_ENTRIES` |
| Max uncompressed total | 8 GiB | `COMIC_ARCHIVE_MAX_TOTAL_BYTES` |
| Max single entry uncompressed | 512 MiB | `COMIC_ARCHIVE_MAX_ENTRY_BYTES` |
| Max compression ratio (per-entry) | 200 : 1 | `COMIC_ARCHIVE_MAX_RATIO` |
| Max archive-within-archive nesting | 1 | `COMIC_ARCHIVE_MAX_NESTING` |

Entry-name validation (zip-slip and friends): reject any entry whose canonicalized path escapes the archive root, contains `\0`, contains control characters, is absolute, is a symlink, or refers to a device file. Validation runs before any byte is materialized.

Subprocess limits for `unrar` (CBR) and `7z` (only if used as fallback):
- `prlimit` wall-time 60 s, RSS 1 GiB.
- stdout cap 512 MiB; stderr cap 1 MiB.
- argv-only invocation (no shell), `--` separator before any user-derived path.
- Working directory = a fresh per-job tempdir under `/data/work/<job-id>/`, removed on exit.
- Subprocess crashes counted; >5 in 10 min on the same archive = quarantine the archive (per-issue flag, no auto-retry).

### 4.2 ComicInfo.xml (Anansi Project schema)
Parse and surface **all** fields. Treat as authoritative when present. Display every populated field on the issue detail page. Fields:

```
Title, Series, Number, Count, Volume, AlternateSeries, AlternateNumber,
AlternateCount, Summary, Notes, Year, Month, Day, Writer, Penciller, Inker,
Colorist, Letterer, CoverArtist, Editor, Translator, Publisher, Imprint,
Genre, Tags, Web, PageCount, LanguageISO, Format, BlackAndWhite, Manga,
Characters, Teams, Locations, ScanInformation, StoryArc, StoryArcNumber,
SeriesGroup, AgeRating, Pages (per-page metadata: Image, Type, DoublePage,
ImageSize, Key, Bookmark, ImageWidth, ImageHeight), CommunityRating,
MainCharacterOrTeam, Review, GTIN
```

### 4.3 series.json (Mylar3 schema)
Parse and use for series-level metadata when present at series root. Fields:

```
metadata: { type, name, description_text, description_formatted,
publisher, imprint, comic_image, year_began, year_end, total_issues,
publication_run, status, booktype, age_rating, comicid, ... }
```

Merge precedence: **per-issue ComicInfo.xml > series.json > inferred from filename**.

### 4.4 MetronInfo.xml
Parse alongside ComicInfo.xml. Treat as additional metadata source (richer creator/credit data). When both exist, MetronInfo wins for fields it defines.

### 4.5 EPUB "comic-style" predicate
An `.epub` is treated as a comic if and only if **all** of:
1. The OPF manifest declares `<meta property="rendition:layout">pre-paginated</meta>`.
2. ≥ 80 % of spine items reference an image resource (`image/svg+xml`, `image/jpeg`, `image/png`, `image/webp`, `image/avif`).
3. Average words-per-page across HTML spine items < 50 (cheap text extract).

Otherwise the file is rejected with a typed error (`issue.not_comic_style_epub`) and excluded from the library. SVG references inside an EPUB are rendered (browser-side) but never executed (CSP forbids inline script anyway; see §17.4).

### 4.6 Encrypted / password-protected archives
Detected at scan time (ZIP general-purpose flag bit 0; RAR encrypted-header flag; 7z AES-256 stream). Action: skip extraction, mark issue `state = encrypted`, surface in admin UI as "X issues are encrypted and cannot be read." Never prompt for a password, never store one.

### 4.7 Filename inference (fallback)
Pattern: `Series Name (Year) #001 (of 12) (Publisher) (Scanner).cbz`. Use a tested parser (port of mylar3's logic). Always overridable.

---

## 5. Library Model

### 5.1 Core entities

```
Library 1──* Series 1──* Issue
Library *──* User    (via library_user_access — see §5.1.1)
User 1──* ProgressRecord, Bookmark, Review
```

- **Library** — root path, scan schedule, default language, default reading direction.
- **Series** — derived from folder structure or `series.json`. Auto-merged on rescan via `comicid` or normalized name+year. Stores aggregated metadata (publisher, year_began, year_end, status, total_issues, age_rating, summary). External-ID columns present from day one even if enrichment is deferred: `comicvine_id BIGINT NULL`, `metron_id BIGINT NULL`, `gtin TEXT NULL`.
- **Issue** — single archive file. Stable ID = `blake3(path)` or `blake3(content)` if `dedupe_by_content=true` (**default**, see §5.1.2). Holds the parsed ComicInfo blob plus extracted/normalized columns for indexing. External-ID columns: `comicvine_id BIGINT NULL`, `metron_id BIGINT NULL`, `gtin TEXT NULL`, `web_url TEXT NULL` (parsed from ComicInfo `<Web>`).
- **Page** — *not a separate table.* Page list and per-page metadata (from ComicInfo `<Pages>`) stored as JSONB on the Issue. Only materialized as rows if a future feature requires it.

### 5.1.1 Library access control (multi-family use case)
Schema lands in **Phase 1**; admin UI lands in **Phase 5**. Until the UI exists, all library access is granted by default and `age_rating_max` is `NULL` (unrestricted). Schema:

```
library_user_access
  library_id UUID,
  user_id UUID,
  role ENUM ('reader','curator'),         -- 'curator' can edit collections in this library
  age_rating_max TEXT NULL,                -- ComicInfo AgeRating cap; NULL = unrestricted
  created_at, updated_at,
  PRIMARY KEY (library_id, user_id)
```

Every series/issue/page/search/OPDS query filters via `WHERE EXISTS (SELECT 1 FROM library_user_access WHERE …)`. The predicate is baked into each raw-SQL allowlist query (§20.1) and into the SeaORM repositories. Search facets and OPDS feeds also respect this filter — never leak metadata for a series the requesting user cannot access.

`age_rating_max` mapping uses ComicInfo `<AgeRating>` ordinal (`Everyone < Everyone 10+ < Teen < Teen+ < Mature 17+ < Adults Only 18+ < X18+`). Issues whose `AgeRating` exceeds the user's cap are filtered out.

### 5.1.2 Issue stable ID
`dedupe_by_content=true` is the **default**. Reasons:
- File renames preserve progress (path-hash would lose it).
- Identical content across two paths (legitimate dupes after a library reorg) collapse to one issue.
- Re-scan after a library move is free.

Trade-off: hashing every file on first scan is I/O-heavy. Mitigation: hash is BLAKE3, parallelized across CPU cores; later scans only re-hash if `(size, mtime)` changes. The `(size, mtime)` shortcut is allowed because the threat model assumes the library root is read-only to the app.

### 5.2 Relationship tables

```
collections                     reading_lists (incl. CBL imports)
  id, owner_id, name,             id, owner_id, name, source ('manual'|'cbl'|'mylar'),
  visibility ('private'|         description, created_at, updated_at
  'shared'|'public'),           reading_list_entries
  cover_issue_id, ...             id, list_id, position (int, gapped),
collection_members                series_id (nullable), issue_id (nullable),
  collection_id, series_id        volume_override, number_override,
  (or issue_id), position,        note, year_hint
  added_at                        UNIQUE (list_id, position)
  UNIQUE (collection_id,
   series_id|issue_id)

series_relationships            story_arcs
  id, from_series_id,             id, name, normalized_name,
  to_series_id,                   first_seen_in_issue_id
  type ENUM (                   story_arc_entries
    'sequel','prequel',           arc_id, issue_id, arc_number (decimal)
    'side_story','spin_off',      UNIQUE (arc_id, issue_id)
    'alternative','adaptation',
    'contained_in','contains',  reviews
    'shares_universe',            id, author_id, target_type ('series'|'issue'),
    'crossover_with'),            target_id, rating (0-10 nullable),
  metadata JSONB,                 title, body, contains_spoilers BOOL,
  created_at,                     created_at, updated_at
  UNIQUE (from, to, type)         UNIQUE (author_id, target_type, target_id)
  CHECK (from != to)
```

(No `review_reactions` table in v1 — community features are deferred. See §17 Backlog.)

**Series relationships** are directed and typed. Inverse pairs (`sequel`/`prequel`, `contains`/`contained_in`, `spin_off`/`spun_off_from`) are auto-created in a transaction so traversal is symmetric without UNION queries. Use a recursive CTE wrapped in a SeaORM raw query for "everything connected to series X" — one of the explicit raw-SQL escape hatches.

### 5.3 CBL (Comic Book List) import
- Standard `.cbl` is XML: `<ReadingList><Books><Book Series="..." Number="..." Volume="..." Year="..."/></Books></ReadingList>`.
- **Import is a multi-step wizard, not a one-shot operation.** Never silently create unresolved entries.
  1. **Parse** the `.cbl` file; extract all `<Book>` references.
  2. **Auto-match** each entry against the library using normalized name + year + volume. Track confidence: high (exact match), medium (fuzzy match), none.
  3. **Mapping screen** — present every entry to the user with the auto-match result. For high-confidence matches, pre-select. For medium, show the top 3 candidates. For unmatched, show a "search library" picker plus a "skip this entry" option.
  4. **Confirm & create** — only after user reviews. The reading list is created in a single transaction with all entries resolved or explicitly skipped.
- **No ghost rows.** Skipped entries are dropped from the list (with a count surfaced in the wizard summary so the user knows what was excluded).
- **Re-resolution:** on subsequent library scans, if a previously skipped CBL entry's target series now exists, surface a notification offering to add it. User opts in.
- Export: regenerate `.cbl` from `reading_list_entries` ordered by position.

### 5.4 Story arcs
- Auto-built from ComicInfo `StoryArc` + `StoryArcNumber` on scan. Multiple arcs per issue supported (ComicInfo allows comma-separated).
- Manually editable; user edits never overwritten by re-scan (sticky flag per entry).

### 5.5 Collections vs Reading Lists vs Story Arcs
- **Collection** — user's bookshelf grouping (unordered or loosely ordered). Members are series **or** issues.
- **Reading List** — strict order. Members are issues. CBL-compatible.
- **Story Arc** — narrative grouping derived from metadata. Auto-maintained.

### 5.5.1 Series completion semantics
A series surfaces a `completion_state` derived per-user at query time:

- `count(read_issues_in_series) == series.total_issues` AND `series.status == 'ended'` → **`complete`**
- `count(read_issues_in_series) == series.total_issues` AND `series.status == 'continuing'` → **`caught_up`** (rendered as "Caught up — N issues read; series ongoing")
- `count(read_issues_in_series) > 0` AND `< series.total_issues` → **`reading`**
- `count(read_issues_in_series) == 0` → **`unread`**

`series.status` is sourced from `series.json` (`metadata.status` field: `'continuing' | 'ended' | 'cancelled' | 'hiatus'`); when missing, defaults to `'continuing'` (the conservative choice — never claims `complete` without explicit confirmation).

`series.total_issues` is sourced from `series.json` `metadata.total_issues` when present; otherwise inferred as the count of known issues in the series (which makes `complete` unreachable until external metadata fills it in — by design).

Specials, annuals, and one-shots are counted only if their ComicInfo `<Format>` is in the user's "count toward completion" preference set (default: only `Series` issues count; `Annual`, `Special`, `One-Shot` excluded). Per-series override available.

### 5.6 Reviews
- Per-issue and per-series reviews supported. User has at most one review per target.
- **Permissions:** author can edit/delete their own review. Server admin can delete any review (no edit, only delete with reason logged). No community flagging in v1.
- **Spoiler tags:** optional `contains_spoilers` boolean. Renders as a click-to-reveal blur in the UI when true. Not enforced.
- Ratings (0-10) are stored on the review row; rating-only entries (no body) are valid.

### 5.7 Relationship suggestions
- Background job runs after every library scan, generating *candidate* relationships — never directly creating edges.
- Candidates stored in `relationship_suggestions` table:
  ```
  id, from_series_id, to_series_id, suggested_type,
  confidence (0.0-1.0), evidence JSONB,
  status ('pending'|'accepted'|'rejected'|'modified'),
  created_at, reviewed_at, reviewed_by_user_id
  ```
- **Sources of evidence (`evidence` JSONB):**
  - **ComicInfo fields** — `AlternateSeries` references on issues in series A pointing to series B → suggest `crossover_with`. `SeriesGroup` shared between A and B → suggest `shares_universe`.
  - **Filename heuristics** — folder structure like `Series Name/Vol 2/` → suggest `sequel` of `Series Name/Vol 1/`. Numbered series (`X-Men (2019)` → `X-Men (2021)`) → suggest `sequel`.
  - **External enrichment (optional, future)** — Comic Vine API or Metron API match → suggest with their relationship data.
- **Review UI** — admin-only screen lists pending suggestions, grouped by confidence, with full evidence shown. Bulk-accept high-confidence, individual review for medium/low. Rejected suggestions stay rejected (re-suggesting requires manually clearing the rejection).
- Status transitions are append-only audit trail: never delete suggestions, just mark status.

### 5.8 Indexing notes (non-search)
- `series.normalized_name` (lowercased, punctuation-stripped) indexed for join lookups and CBL matching.
- Partial index on `progress_records (user_id, updated_at) WHERE updated_at > now() - 30d` for sync delta queries.
- `library_user_access (user_id)` for the ACL `EXISTS` predicate.
- `issues (series_id, sort_number)` for series-detail listing.
- `issues (content_hash) UNIQUE` when `dedupe_by_content=true`.
- Search-related indexes are documented in §6.

### 5.9 Audit log (admin actions)
A unified `audit_log` table records every admin-visible action. Append-only; no UPDATE or DELETE.

```
audit_log
  id UUID PK,
  actor_id UUID NOT NULL,                  -- the user who performed the action
  actor_type ENUM ('user','app_password','system'),
  action TEXT NOT NULL,                    -- dotted: 'review.delete', 'app_password.create'
  target_type TEXT NULL,                   -- 'review','user','library','issue','suggestion',…
  target_id TEXT NULL,                     -- the target's stable id
  payload JSONB NOT NULL DEFAULT '{}',     -- before/after, reason, scope, …
  ip INET,
  user_agent TEXT,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now()
```

Logged actions (initial set; expandable): `review.delete`, `user.invite`, `user.disable`, `user.role_change`, `library.create`, `library.update`, `library.scan_trigger`, `app_password.create`, `app_password.revoke`, `suggestion.accept`, `suggestion.reject`, `suggestion.modify`, `oidc.config_update`, `library_user_access.grant`, `library_user_access.revoke`, `share_link.create`, `share_link.revoke`, `data_export.request`, `account.delete`, `opds.download`.

Retention: forever (volume is small). Reads via `GET /admin/audit?actor=&action=&target=&since=&cursor=&limit=` — admin only. The endpoint never returns secrets in `payload`.

---

## 6. Search

### 6.1 Backend choice
- **Postgres-only** for v1. `tsvector` + `pg_trgm` + `unaccent` + `fuzzystrmatch` cover every search feature listed below at the scale this app realistically reaches (≤100k issues, ≤10k series, low thousands of reviews/lists).
- Meilisearch is the documented upgrade path if instant search, advanced typo handling, or large-scale needs ever justify a separate service. The unified search API (§6.4) is designed so that swap requires no client changes.

### 6.2 Required Postgres extensions (enable in initial migration)
```sql
CREATE EXTENSION IF NOT EXISTS pg_trgm;        -- trigram fuzzy match, autocomplete
CREATE EXTENSION IF NOT EXISTS unaccent;       -- "Pokémon" matches "pokemon"
CREATE EXTENSION IF NOT EXISTS fuzzystrmatch;  -- levenshtein for "did you mean"
-- pgvector deferred until/unless semantic search is added
```

### 6.3 Searchable corpus per entity
Each searchable entity has a `search_doc tsvector` generated column with weighted fields. Weights drive ranking: A (highest) → D (lowest).

| Entity | A (name/title) | B (key metadata) | C (descriptive) | D (bulk text) |
|---|---|---|---|---|
| **Series** | name, alternate names | publisher, year, characters, teams, locations | tags, genres, story arcs | summary |
| **Issue** | title, series name | issue number, characters, teams, locations | tags, story arcs, creators (writer/artist) | summary |
| **Review** | title, target series/issue name | author username | — | body |
| **Collection** | name | owner username | description | included series names (denormalized) |
| **Reading list** | name | source ('cbl'/'manual'), owner username | description | included series names (denormalized) |
| **Bookmark** | issue title, series name | page number | label | note |

**Implementation:** generated columns on each entity table, computed at write time. For collection/reading list, "included series names" requires a trigger that updates the parent's `search_doc` when membership changes. Document this trigger in the migration.

**Excluded by design:** ComicInfo `<Notes>` is **not** indexed — many community releases stuff scan-tool details there which pollute search results. The field is still surfaced on the issue detail page; it's only excluded from the search corpus.

### 6.4 Unified search API
```
GET /search
  ?q=<query>
  &types=series,issue,review,collection,reading_list,bookmark   # default: all
  &library=<id>                                                 # scope to one library
  &mine_only=true                                               # restrict to user's content
  &limit=20&offset=0
```

Response is a typed, ranked, mixed list:
```json
{
  "query": "civil war",
  "total": 47,
  "results": [
    { "type": "series", "id": "...", "score": 0.94, "snippet": "...", "data": {...} },
    { "type": "reading_list", "id": "...", "score": 0.88, ... },
    { "type": "issue", "id": "...", "score": 0.81, ... }
  ],
  "facets": {
    "type": { "series": 3, "issue": 38, "reading_list": 1, "review": 5 },
    "publisher": { "Marvel": 41, "DC": 3, ... }
  },
  "did_you_mean": null
}
```

**Implementation pattern:** typed sub-queries per entity (each using its own `tsvector` index), `UNION ALL`'d, scores normalized to 0-1 within each type then weighted globally (e.g., series matches outrank issue matches at equal raw score). Single SQL query in the raw-SQL escape hatch.

**Hardening (apply to every search endpoint):**
- Use `websearch_to_tsquery(<lang>, $1)` — never `to_tsquery` on user input. `websearch_to_tsquery` is parser-tolerant and does not raise on malformed syntax.
- Cap query length at 200 chars before any DB call; longer = 400.
- Run search queries under a dedicated Postgres role with `statement_timeout = 2s` and `idle_in_transaction_session_timeout = 5s`.
- Apply the §5.1.1 ACL predicate to every per-entity sub-query so a search never reveals a series the requester cannot access.

### 6.5 Per-context endpoints (specialized)
The unified endpoint covers cross-entity discovery. Specialized endpoints exist for in-context search where mixed results would be wrong:

```
GET /series?q=<query>                        # library list filter, autocomplete
GET /series/{id}/issues?q=<query>            # within-series search
GET /reviews?q=<query>&series_id=<id>        # review search within a series
GET /search/autocomplete?q=<prefix>          # prefix-only, top 10, sub-50ms
```

Autocomplete uses `pg_trgm` similarity ordering with a `LIMIT`, no `tsvector` involved (faster for short prefixes).

### 6.6 Ranking and boosts
- Base ranking: `ts_rank_cd(search_doc, query, 32)` (32 = normalize by document length).
- Boosts applied multiplicatively after base rank:
  - **+50%** if entity is in user's library / owned by user.
  - **+25%** if user has read or reviewed it (recency-weighted: full boost for last 30 days, decaying to zero at 1 year).
  - **+10%** if entity has been accessed (any interaction) in last 7 days.
- Final ordering: `score DESC, updated_at DESC`.

### 6.7 Multi-language support
- `language_code` column on each indexable entity (3-letter ISO 639-2; default `eng`).
- Generated `search_doc` chooses Postgres text-search config based on language_code: `english`, `french`, `german`, `spanish`, `simple` (fallback for unsupported including `jpn`, `zho`).
- Query side: detect query language heuristically or pass `?lang=` param; default to user's preferred language list.
- **Limit:** Japanese/Chinese fall back to `simple` config (no stemming, exact token match only). Acceptable for v1; revisit if Japanese manga search becomes a pain point.

### 6.8 Typo tolerance
- Two-stage: full-text query first (fast, exact). If <3 results, run a `pg_trgm` similarity query as fallback and merge.
- "Did you mean" suggestions powered by `levenshtein` against a curated dictionary table built from existing series/character/creator names. Populated by background job after each scan.

### 6.9 Highlighting
- `ts_headline()` for snippets in results. Cap at 200 chars per snippet, max 3 fragments per result.
- Frontend renders snippets with a thin wrapper around `<mark>` tags from `ts_headline`.

### 6.10 Search analytics (deferred)
- Capture query text + result count + clicked result in a `search_log` table for later tuning. Keep the schema minimal; don't build analytics UI in v1.
- **Retention: rolling 90 days.** A nightly apalis job deletes rows older than `now() - interval '90 days'`. Avoids unbounded growth and reduces privacy footprint.

---

## 7. Reader

### 7.1 Modes
- **Single page**, **double page** (auto-detect from `DoublePage` flag and aspect ratio), **vertical webtoon** (continuous scroll), **long-strip**.
- **Reading direction:** LTR, RTL (auto from `Manga=YesAndRightToLeft`), TTB.
- **Fit modes:** width, height, original, smart (crop margins via edge detection).

### 7.2 Performance
- Preload N+2 / N-1 pages; decode off-main-thread (Web Worker / Swift Task / Kotlin coroutine).
- **Web decode pipeline:** `fetch(page) → blob → createImageBitmap(blob, { colorSpaceConversion: 'none', resizeQuality: 'high' })` inside a Web Worker. Result `ImageBitmap` is transferred to the main thread and painted via `OffscreenCanvas`/`<canvas>`. WebCodecs `ImageDecoder` is *not* used — overkill for static images and lacking JXL support across browsers.
- **Format support:** browsers handle JPEG/PNG/WebP/GIF natively; AVIF works in current Safari/Chromium/Firefox; **JXL is transcoded server-side to AVIF (or WebP fallback) at scan time**; the original is preserved for OPDS download. Cache key for the transcoded variant is content-addressed: `/data/transcoded/<blake3-of-source>.avif`.
- AVIF/WebP for thumbnails; original (or transcoded) bytes streamed for full pages.
- HTTP `Range` requests on archive entries; server keeps a small LRU of open ZIP central directories — see §7.2.1.

### 7.2.1 ZIP central-directory LRU
The library service holds a small LRU of `(issue_id → (File handle, parsed central directory))` so repeat page reads of a hot issue avoid re-parsing.

- **Default capacity:** 64 entries (≈ 64 open FDs + a few KB each). Tune via `COMIC_ZIP_LRU_CAPACITY`.
- Eviction policy: LRU; eviction **must close the FD** (`Drop` impl on the holder).
- Prometheus metrics: `comic_zip_lru_open_fds` (gauge), `comic_zip_lru_evictions_total` (counter), `comic_zip_lru_hits_total` / `_misses_total`.
- The reader's prefetch pattern (N+2 / N-1) means the LRU should comfortably fit the working set of 50 concurrent readers each on one issue.
- For deflated entries (rare; CBZ is usually stored), Range requests on the page byte endpoint require decompression-from-start. Server handles correctly but logs at `debug` so that misconfigured archives are visible.

### 7.3 UX
- Tap zones (configurable), keyboard shortcuts, gesture customization.
- Mini-map / page strip overlay.
- Color modes: normal, sepia, OLED black, custom curves.
- Per-series remembered settings; per-user **default reading direction** (LTR/RTL/TTB) honored when ComicInfo doesn't specify.

### 7.4 Accessibility (commitment, not aspiration)
- Target: WCAG 2.2 AA across the entire web app.
- Reader keyboard model: `←/→` (or `↑/↓` in TTB) page nav; `Space` next page; `Esc` exit; `m` toggle minimap; `f` fit-mode cycle. Page changes announced via an `aria-live="polite"` region: "Page 12 of 28."
- Tab traversal of reader chrome only when chrome is visible; chrome auto-hides during read flow.
- `prefers-reduced-motion` respected — page-turn animations skipped, instant swap.
- Screen-reader content: per-page alt text from ComicInfo `<Pages>` if present; user-supplied alt text per-page (Phase 5+).
- Spoiler-blur (§5.6) is purely visual: `aria-hidden="false"`, content is readable to assistive tech, blur applied via CSS filter only.
- Color contrast verified for sepia and OLED-black themes via `axe-core` in CI from Phase 2 onward.
- A focused **accessibility audit milestone (Phase 3.5)** runs NVDA + VoiceOver passes before reader v2 ships.

---

## 8. OPDS

- **OPDS 1.2** + **OPDS 2.0 (JSON)**, both at `/opds/v1/...` and `/opds/v2/...`.
- Endpoints: root catalog, series, recent, collections, search (OpenSearch), reading lists.
- Page Streaming via OPDS-PSE for compatible readers (Chunky, Panels, Yomu).
- Auth: HTTP Basic with **app password** (separate from main creds), or Bearer token.
- Scope: OPDS feeds respect §5.1.1 library ACLs and the requesting credential's scope (§8.2).

### 8.1 App-password storage
- Generated server-side: 32 chars, base32 (`Crockford`). Shown **once at creation**; never retrievable. UI provides "copy to clipboard" plus QR code.
- Hashed at rest with **argon2id** (`m=64 MiB, t=3, p=1`), salt per password, plus a server-wide pepper at `/data/secrets/pepper` (32 bytes, generated on first start, never logged).
- HTTP Basic username = the user's stable id (UUID v7); password = the app token.
- Stored row: `app_passwords (id, user_id, label, scope, hash, created_at, last_used_at, last_used_ip, revoked_at)`.
- Brute-force defense: failed Basic-Auth attempts counted in the `failed-auth` rate limit bucket (§17.7). After 10 failures from one IP in 1 min, that IP is 401-locked for 15 min on OPDS specifically.
- Rotation: users rotate by creating a new password and revoking the old. No silent rotation.

### 8.2 App-password scopes
Two presets at creation time (radio button):
- **`read`** — list catalogs, fetch metadata, fetch page bytes, OPDS-PSE. Cannot write progress, ratings, reviews, or any user state.
- **`read+progress`** — `read` plus progress writes (last-page-read, finished). Cannot write reviews, ratings, collections, app passwords, or admin actions.

Granular scopes are deferred to backlog (§21) — two presets cover Chunky, Panels, Yomu, KOReader workflows.

### 8.3 Signed URLs (OPDS-PSE)
OPDS-PSE clients sometimes mishandle per-request auth headers across redirects. Page byte URLs surfaced in OPDS-PSE feeds are signed:

```
/opds/pse/{issue_id}/{page}?sig=<HMAC>&exp=<epoch>&u=<app_password_id>
```

Signature = HMAC-SHA256 over `(issue_id, page, exp, u)` with a server-side key (`/data/secrets/url-signing-key`, 32 bytes, generated on first start). `exp` defaults to 24 h from issuance. Verification path: HMAC compare → `exp > now()` → app-password not revoked → ACL check → serve. No DB lookup of the JWT/cookie path is needed; this stays under the §18 page-byte target.

---

## 9. Sync

### 9.1 What syncs
- Read progress (issue, page, percent, timestamp, device).
- Bookmarks, ratings, finished-state.
- Collections, reading list membership.
- Per-series reader settings.

### 9.2 How
- **Automerge 2.x** documents per user, partitioned by domain (one doc for progress, one for collections-membership cache, one for reader settings) to keep document size manageable. Shared-collection *content* is owner-authoritative (§9.5).
- Server stores authoritative Automerge documents in Postgres (BYTEA column on `automerge_documents`, with separate `automerge_changes` table for incremental sync).
- Clients keep local Automerge replicas. On connect, exchange compressed change sets (Automerge's sync protocol) over WebSocket. Falls back to HTTP polling if WebSocket fails.
- Offline-first: all reads and writes work locally. Changes auto-merge when reconnected.
- **"Last page read" semantics:** Automerge's default LWW is overridden with custom merge rule — `max(page)` wins per issue, so progress never goes backward across devices.

### 9.3 Compaction & growth bounds
Automerge documents accumulate change history; without compaction they grow unbounded.

- **Compaction trigger** (whichever fires first per document):
  - History depth > 10 000 changes, **or**
  - On-disk size > 4 MiB, **or**
  - Daily idle window (per document, randomized within 02:00–06:00 server-local).
- **Compaction action:** `Automerge::load() → Automerge::save()` produces a fresh snapshot. Old `automerge_changes` rows older than the latest snapshot are kept 30 days then deleted (forensic window for diagnosing sync bugs).
- **Sharding:** progress doc is sharded per ~10 000 issues (`progress_<shard>`). Reader settings and collections-membership stay single-doc per user.
- **Storage:** documents > 16 KiB compressed via `lz4` at the app layer before BYTEA insert; smaller blobs stored raw. TOAST behavior is acceptable for the sub-MiB range.

### 9.4 DoS bounds (server enforces; client errors loud and reconnects)
- Per-document max BYTEA size: 16 MiB. Reject larger writes with 413.
- Per-WebSocket-message max: 1 MiB. Close 1009 (message too big) on violation.
- Changes per minute per WebSocket: 200. Excess closes 1013 (try again later).
- Concurrent WebSocket connections per user: 8. New connections beyond cap close oldest.

### 9.5 Shared-collection sync semantics (v1)
`visibility = 'shared'` collections (§5.5) are **owner-authoritative**:
- Owner's per-user collections doc holds the canonical membership.
- Members read via REST (`GET /collections/{id}`), do not replicate the doc.
- Member-side actions (e.g., adding their own bookmark) live in their own per-user docs.
- Trade-off: members cannot edit shared collections offline. Acceptable for v1; per-collection Automerge docs are a Phase 7+ enhancement (§21).

### 9.6 WebSocket auth handshake
Browsers cannot set `Authorization: Bearer …` on `new WebSocket(url)`. Auth flow:
1. Authenticated HTTP `POST /ws/ticket` (cookie-authenticated) returns `{ ticket: <opaque-32B>, expires_in: 30 }`.
2. Client opens `wss://…/ws?ticket=<…>` within 30 s. Server validates the ticket once, then forgets it.
3. Cookie-based session is **not** re-checked on the WS upgrade; the ticket is the credential. This keeps reverse-proxy cookie/CORS quirks from breaking the upgrade.
4. Tickets are user-scoped and single-use; ticket validation rate-limited at 30/s per IP.

### 9.7 Phase 2 → Phase 4 progress migration
Phase 2 stores progress in `progress_records` table directly. Phase 4 introduces Automerge docs.

- **Backfill:** on first authenticated connect from a Phase-4-aware client, the server constructs a fresh Automerge progress doc from the user's existing `progress_records` rows and persists it.
- **Cutover:** the `POST /progress` endpoint version is bumped (`/v2/progress` style or via header negotiation). Phase 2 clients receive 410 Gone; users are prompted to refresh the web app (PWA shell will auto-update).
- **Fallback window:** `progress_records` table kept read-only as truth for 90 days post-cutover. After 90 days, dropped in a follow-up migration.
- **Test:** a CI integration test seeds a Phase-2-style DB, runs the upgrade, runs the new client against it, and asserts progress matches.

---

## 10. API Surface (sketch)

OpenAPI spec is the contract. Key routes:

```
# Auth
POST   /auth/oidc/callback
POST   /auth/local/register                    → local-mode only; queues verification email if SMTP set
POST   /auth/local/login                       → local-mode only; rate-limited per §17.7
POST   /auth/local/totp                        → second factor when enrolled
GET    /auth/local/verify-email?token=         → activates pending account
POST   /auth/local/resend-verification         → re-sends verification email
POST   /auth/local/request-password-reset      → always 202; SMTP required
POST   /auth/local/reset-password              → completes reset (TOTP if enrolled)
POST   /auth/refresh                           → rotate refresh token; sets new cookie
POST   /auth/logout                            → revoke refresh; clear cookie
GET    /auth/me                                → current user + roles + csrf token

# Library / browse
GET    /libraries
POST   /libraries/{id}/scan
GET    /series?library=&search=&sort=&page=
GET    /series/{id}
GET    /series/{id}/issues
GET    /issues/{id}                       → full metadata
GET    /issues/{id}/pages                 → page list with dimensions
GET    /issues/{id}/pages/{n}             → bytes (Range supported, content sniffed)
GET    /issues/{id}/pages/{n}/thumb       → WebP

# User state
GET    /progress?since=<timestamp>        → sync delta
POST   /progress                          → batch upsert
GET    /collections, /reading-lists, ...

# Search
GET    /search?q=&types=&library=&mine_only=  → unified ranked results (§6.4)
GET    /search/autocomplete?q=                → prefix-only, fast (§6.5)

# Sync
POST   /ws/ticket                         → 30s single-use ws auth ticket (§9.6)
WS     /ws?ticket=                        → progress, scan events

# OPDS
GET    /opds/v1, /opds/v2
GET    /opds/pse/{issue_id}/{n}?sig=&exp=&u=  → signed PSE page bytes (§8.3)

# Admin (role: admin)
GET    /admin/audit?actor=&action=&since=&cursor=&limit=
GET    /admin/users
POST   /admin/users/{id}/role
POST   /admin/users/{id}/disable
GET    /admin/library-access
POST   /admin/library-access            → grant/revoke library_user_access
GET    /admin/encrypted-issues          → list of issues marked state='encrypted'
GET    /admin/relationship-suggestions  → §5.7
POST   /admin/import/progress           → JSON import for Komga/Kavita migration

# App passwords (per-user)
GET    /me/app-passwords
POST   /me/app-passwords                → returns plaintext token once
DELETE /me/app-passwords/{id}

# Self-service data rights (§17.10)
GET    /me/export                        → JSON download of all user data
DELETE /me                               → cascade delete; admin notified

# Sharing
POST   /share/issue/{id}                 → create share link (returns /s/{token})
DELETE /share/{id}
GET    /s/{token}                        → public reader page (no auth)

# Ops
GET    /healthz                          → 200 ok / 503 during shutdown or dep failure
GET    /readyz                           → 200 once migrations applied & deps ready
GET    /metrics                          → Prometheus
POST   /metrics-rum                      → web RUM ingestion (§18.2)
```

All write endpoints accept either a session cookie + `X-CSRF-Token` (web) or `Authorization: Bearer <jwt>` (mobile/API). Endpoints that mutate global state require admin role; per-user endpoints require ownership.

---

## 11. Mobile Apps (deferred)

Mobile apps are **not in v1**. The build priority is server + web first; mobile follows after Phase 7. This section captures intent so server/API decisions stay compatible with mobile needs.

### 11.1 iOS — Phase 8 target
- **Minimum: iOS 17.** Acceptable trade-off for a 2026 build — covers > 90 % of devices in active use and unlocks SwiftData, Observable macro, and TipKit. Older devices fall back to web reader on Safari.
- SwiftUI + SwiftData for local cache.
- OIDC login via ASWebAuthenticationSession.
- Reader feature parity with web (single, double, RTL, webtoon).
- Background download of next N issues in series for offline reading.
- Files app integration (import `.cbz`).
- ProMotion-aware page transitions; haptics on page turn.
- iPad: split view library/reader. Apple Pencil annotations is a stretch goal within Phase 8.
- Automerge sync via Swift bindings.

### 11.2 Android — Phase 9 (aspirational)
- Kotlin Multiplatform skeleton, sharing networking + models with potential future desktop.
- Jetpack Compose, Material 3.
- Foreground service for downloads. Storage Access Framework for sideload.
- Same feature scope as iOS.
- May never ship.

### 11.3 Cross-platform notes
- Sign-in via OIDC on both platforms (ASWebAuthenticationSession on iOS, Custom Tabs on Android).
- Biometric lock for adult content (deferred to backlog, §21).
- Cast / external display support (deferred to backlog).

---

## 12. Self-Host & Ops

The server ships as a single Docker image plus optional Postgres and Redis. The goal is "one `docker compose up` and you have a working install."

### 12.1 Production-style Docker Compose
- File: `compose.prod.yml` — what users will deploy. Mirrors what real installs look like.
- Services:
  - `app` — the comic reader server (Rust binary + bundled Next.js standalone). Single container, multi-stage build, **two-process model** (§12.1.2).
  - `postgres` — official `postgres:17-alpine`. Volume-mounted data dir.
  - `redis` — official `redis:7-alpine`. **Effectively required** in prod for the `apalis` job framework (scan, thumbnails, suggestions). The app starts without Redis only in `just dev`.
- Library volume: read-only mount at `/library` (configurable). The app must never write here. Startup check: open a tempfile under `/library`; if it succeeds, log a `warn` and refuse to start unless `COMIC_LIBRARY_ALLOW_WRITABLE=true` is set.
- Data volume: writable at `/data` for thumbnails (content-addressed, see §17.5), generated transcoded pages, search dictionaries, work tempdirs, secrets.
- Healthcheck `/healthz`: returns 200 when Postgres and Redis are reachable; 503 during shutdown or dep failure. Liveness only.
- Readiness `/readyz`: returns 200 once migrations applied and a sample query succeeds. Used by reverse proxies for traffic routing.
- Reverse proxy expectations: app listens on a single port for HTTP+WS. TLS is the user's responsibility (Caddy/Traefik/nginx upstream). Reference Caddyfile below (§12.1.1). Required forwarded headers: `X-Forwarded-Proto`, `X-Forwarded-For`, `X-Forwarded-Host`. The `COMIC_TRUSTED_PROXIES` env var (CIDR list, default empty) controls which upstream IPs the app trusts to set those headers — without it, every request appears to come from `127.0.0.1` for rate-limit purposes.

### 12.1.1 Reference Caddyfile
The canonical reference also lives at `docs/install/caddy.md` for users who don't read the spec. Caddy is the recommended reverse proxy because it gives automatic ACME certificates, HTTP/3, and correct `X-Forwarded-*` defaults out of the box.

```caddyfile
# /etc/caddy/Caddyfile
{
    # Cluster-wide options
    email admin@example.com           # ACME contact
    servers {
        protocols h1 h2 h3            # enable HTTP/3 for page-byte multiplexing
    }
}

comics.example.com {
    # Compress responses (page bytes are pre-compressed images, but JSON & HTML benefit)
    encode zstd gzip

    # Security headers Caddy can set in addition to the app's own
    # (the app sets the full §17.4 set; these are belt-and-braces)
    header {
        Strict-Transport-Security "max-age=63072000; includeSubDomains; preload"
        X-Content-Type-Options "nosniff"
        Referrer-Policy "strict-origin-when-cross-origin"
        # Caddy injects forwarded headers automatically; do not duplicate them here
        # Remove any cache-control on auth-y paths
        -Server
    }

    # WebSocket sync — no extra config needed; Caddy proxies WS by default,
    # but be explicit about read/write timeouts since the app sends keepalives.
    @ws {
        path /ws
        header Connection *Upgrade*
        header Upgrade websocket
    }
    reverse_proxy @ws app:8080 {
        transport http {
            read_timeout  10m
            write_timeout 10m
            keepalive 60s
        }
    }

    # OPDS-PSE page bytes — long downloads, allow large request/response buffers
    @opds_pse path /opds/pse/*
    reverse_proxy @opds_pse app:8080 {
        transport http {
            response_header_timeout 30s
            read_timeout  5m
        }
        flush_interval -1            # stream bytes; do not buffer
    }

    # Page bytes for web reader — same streaming behavior
    @page_bytes path_regexp page ^/issues/[^/]+/pages/\d+$
    reverse_proxy @page_bytes app:8080 {
        flush_interval -1
    }

    # Everything else
    reverse_proxy app:8080 {
        # Caddy sets X-Forwarded-For, X-Forwarded-Proto, X-Forwarded-Host automatically.
        # The app must list Caddy's IP in COMIC_TRUSTED_PROXIES.
        header_up Host {host}
        transport http {
            response_header_timeout 30s
        }
    }

    # Optional: redirect bare-host plain HTTP to HTTPS — Caddy does this by default.
}
```

**Notes for operators:**
- Set `COMIC_TRUSTED_PROXIES` to the Docker bridge subnet that Caddy lives on (e.g. `172.18.0.0/16`). Without it, rate limiting (§17.7) sees every request as coming from Caddy and fails open per-IP.
- `flush_interval -1` on the page-byte and OPDS-PSE routes is critical — without it Caddy buffers the response in memory, breaking Range request UX and inflating RSS on large pages.
- `comics.example.com` triggers automatic Let's Encrypt issuance. For air-gapped or self-signed deployments, see Caddy's `tls` directive.
- Caddy auto-enables HTTP/3 once HTTPS is in place; no application change required.
- A nginx equivalent lives at `docs/install/nginx.md` for users who must run nginx; it's longer and less idiomatic, but functionally equivalent.

### 12.1.2 Container process model
The container runs **two processes** under `tini` (PID 1):
- Rust server on `:8080` (HTTP API, OPDS, WS, /metrics).
- Next.js standalone (`node server.js`) on `:3000` for the web UI.
- Rust reverse-proxies the web routes to `:3000` so users see one external port.
- `tini --` reaps zombies and propagates SIGTERM to both. Graceful shutdown order: Rust enters draining state (`/healthz` → 503), waits for in-flight requests + WS closes (cap 30 s), closes DB pool, flushes log buffer; Next.js receives SIGTERM in parallel and drains. Container exits 0 once both children exit.
- Edge runtime is **not** used for any Next.js route. Server Actions are **disabled** in `next.config.js` — all client→server traffic goes through the Rust API via `openapi-fetch`.

### 12.2 Image build
- Multi-stage Dockerfile:
  1. Build stage: Rust workspace compiled with `cargo chef` for layer caching.
  2. Web stage: Next.js built with `output: 'standalone'`.
  3. Final stage: distroless `gcr.io/distroless/cc-debian12` + the Rust binary + Next.js standalone bundle + `unrar` binary copied in.
- Multi-arch: amd64 + arm64. Built via Docker Buildx in CI.
- Tags: `:latest`, `:edge` (main branch), semver tags `:v1.2.3`, `:v1.2`, `:v1`.
- Renovate/Dependabot-friendly versioned tags only — never bare digests in compose files.

### 12.3 Configuration
- All config via environment variables, with optional `config.toml` overlay loaded if present.
- Every secret-bearing var also accepts a `_FILE` suffix that reads from a path (Docker/Podman secrets compatibility): `COMIC_OIDC_CLIENT_SECRET_FILE=/run/secrets/oidc`.
- SIGHUP reloads OIDC + auth config without restart; other config changes require restart.
- Required env vars documented in `.env.example`:
  ```
  # Core
  COMIC_DATABASE_URL=postgres://...
  COMIC_REDIS_URL=redis://...                 # required in prod (apalis); optional in dev
  COMIC_LIBRARY_PATH=/library
  COMIC_DATA_PATH=/data
  COMIC_PUBLIC_URL=https://comics.example.com
  COMIC_LOG_LEVEL=info
  COMIC_TRUSTED_PROXIES=10.0.0.0/8,172.16.0.0/12   # CIDRs whose X-Forwarded-* are trusted

  # Auth mode
  COMIC_AUTH_MODE=oidc                        # oidc | local | both

  # OIDC (when mode includes oidc)
  COMIC_OIDC_ISSUER=https://auth.example.com/application/o/comics/
  COMIC_OIDC_CLIENT_ID=...
  COMIC_OIDC_CLIENT_SECRET=...                # or COMIC_OIDC_CLIENT_SECRET_FILE
  COMIC_OIDC_TRUST_UNVERIFIED_EMAIL=false     # opt-in; logs warn at startup if true

  # Local users (when mode includes local)
  COMIC_LOCAL_REGISTRATION_OPEN=false         # close after first user (admin bootstrap)
  COMIC_LOCAL_REQUIRE_TOTP=false              # if true, every local user must enroll TOTP

  # Tokens (defaults shown)
  COMIC_JWT_ACCESS_TTL=15m
  COMIC_JWT_REFRESH_TTL=30d

  # Limits (defaults shown; full table in §17.7)
  COMIC_RATE_LIMIT_ENABLED=true

  # Observability (default-on)
  COMIC_OTLP_ENDPOINT=                        # empty → stdout exporter only

  # SMTP (optional; enables local self-serve recovery — §17.1.1)
  COMIC_SMTP_HOST=                            # if empty, recovery features disabled
  COMIC_SMTP_PORT=587
  COMIC_SMTP_USERNAME=
  COMIC_SMTP_PASSWORD=                        # or COMIC_SMTP_PASSWORD_FILE
  COMIC_SMTP_TLS=starttls                     # starttls | implicit | none
  COMIC_SMTP_FROM=Comic Reader <noreply@example.com>
  COMIC_SMTP_DKIM_KEY_FILE=                   # optional DKIM signing
  ```
- All env vars use `COMIC_` prefix to avoid collisions.

### 12.3.1 SMTP behavior
- SMTP is **optional**. When `COMIC_SMTP_HOST` is empty, all email-dependent features (local-mode email verification, password reset) are disabled and the corresponding UI is hidden.
- When SMTP is configured, the server validates connectivity at startup with a `NOOP` against the configured host; failure logs a `warn` but does not block startup (transient network issues shouldn't kill the app).
- All outbound mail goes through an apalis job queue with retry (3 attempts, 1 min / 5 min / 30 min backoff). Permanent failures recorded in `audit_log` (`email.send_failed`).
- Volume cap: 100 mail/hour total across the install. Cap is conservative to keep the server out of bulk-sender heuristics on shared SMTP relays. Tunable via `COMIC_SMTP_MAX_PER_HOUR`.

### 12.4 Observability
- **Logs:** structured JSON to stdout. Log level configurable. Trace IDs threaded through requests via `tracing` crate.
- **Metrics:** Prometheus endpoint at `/metrics`. Standard HTTP metrics, scan duration, queue depth (apalis), Postgres pool stats, Automerge document sizes, ZIP-LRU stats (§7.2.1), rate-limit denials, failed-auth counters per IP.
- **Traces:** OpenTelemetry **on by default** with stdout exporter. Set `COMIC_OTLP_ENDPOINT=…` to ship to a collector. `traceparent` always propagated and surfaced in log lines, so users can correlate without an OTLP backend.

### 12.5 Backup & restore
- **Postgres:** standard `pg_dump`. Document a recommended cron-style backup recipe in install docs.
- **`/data` volume:** `tar` snapshot. Most contents regenerable from Postgres + library, but `/data/secrets/` (URL-signing key, app-password pepper) is **not** regenerable — losing it invalidates every existing app password and OPDS-PSE URL. Backups must include `/data/secrets/` and treat it as sensitive (encrypt at rest).
- **Library files:** user's responsibility. The app never modifies them.
- Restore procedure tested in CI: spin up fresh app, restore dump + `/data` tarball, verify scan re-attaches to existing rows by hash and existing app passwords still authenticate.

### 12.6 Migrations
- SeaORM migrator. Run automatically on startup unless `COMIC_AUTO_MIGRATE=false` is set.
- All migrations idempotent and reversible where practical.
- Schema changes require a migration; never rely on entity definitions to mutate schema.
- **Multi-replica caveat:** even though v1 is single-instance, document in `docs/install/scaling.md` that running multiple replicas requires `COMIC_AUTO_MIGRATE=false` and a one-shot init container running `comic-reader migrate`. Two instances racing migrations corrupts schema.

### 12.7 SSO integration (homelab-specific)
- App at `comics.example.com`, OIDC to `auth.example.com`.
- Authentik 2025.10+ has a known `email_verified: false` issue when claims aren't explicitly mapped — fix documented in `docs/install/authentik.md`.
- **Default behavior:** missing `email_verified` claim is treated as **false**. The recommended fix is to add an Authentik scope mapping that emits the user-attribute value. As an opt-in workaround, set `COMIC_OIDC_TRUST_UNVERIFIED_EMAIL=true` — this logs a `warn` on every startup and is appropriate only when the Authentik tenant disables self-service signup.
- Every OIDC login is logged at `info` with `claim_email_verified_present: bool`, `claim_email_verified: bool`, `subject`, `issuer`. PII discipline (§15.4) still applies — the email value itself is not logged.

### 12.8 Default admin bootstrap
- The **first user to authenticate successfully** on a fresh database is granted the `admin` role automatically. The choice is logged at `info` (`first_admin_bootstrap`) and recorded in `audit_log` (`user.role_change`, `actor_type='system'`).
- After that first login, no further admin grants happen automatically; subsequent admins require an existing admin to grant via `POST /admin/users/{id}/role`.
- **Mandatory hardening at install time** — documented as a check in the install guide:
  - In OIDC mode, ensure the Authentik application is restricted to the intended user pool *before* opening the app to the network. Otherwise a stranger could be the "first" user.
  - In local mode, set `COMIC_LOCAL_REGISTRATION_OPEN=true` only long enough to register the admin, then set it to `false` and restart.
  - The first-run install wizard (web UI) prompts for this and refuses to proceed until acknowledged.
- A CLI escape hatch exists: `comic-reader admin-promote --email admin@example.com`. Requires shell access into the container.

---

## 13. Local Development Environment

The goal: a developer (or a Claude Code instance) can clone the repo and have a working dev environment with one command.

### 13.1 Prerequisites
- Rust stable (rustup). Pinned via `rust-toolchain.toml` so `cargo` picks the right version automatically.
- Node 22 LTS or newer, pnpm 9 or 10. Pinned via `.nvmrc` and `package.json` `packageManager` field.
- Docker + Docker Compose (for Postgres + Redis in dev).
- `just` (justfile task runner). Optional but recommended; falls back to documented raw commands.

### 13.2 First-run setup
```
git clone <repo>
cd comic-reader
just bootstrap        # installs tooling, sets up pre-commit hooks, seeds .env from .env.example
just dev-services-up  # starts postgres + redis containers via compose.dev.yml
just migrate          # runs SeaORM migrations
just seed             # loads sample fixture data + sample comics into ./fixtures/library/
just dev              # runs server + web in parallel with hot reload
```

### 13.3 Dev compose file (`compose.dev.yml`)
- Postgres + Redis only. The app itself runs natively for fast iteration. Redis is required even in dev because apalis is the job framework (§3.1).
- Postgres exposes 5432 to host. Volume in `./.dev-data/postgres`.
- Redis exposes 6379 to host. Ephemeral.
- A throwaway "auth-mock" container (Dex or a custom OIDC mock) so devs don't need Authentik running locally. Documented in `docs/dev/oidc-mock.md`. Devs can also run with `COMIC_AUTH_MODE=local` and skip the mock entirely.

### 13.4 Sample data (`./fixtures/`)
- A curated set of **public-domain comics** — early Golden Age titles now in the public domain (e.g., Action Comics #1, Detective Comics #27, Captain America Comics #1). Sourced from Digital Comic Museum / Comic Book Plus / Internet Archive with provenance recorded in `fixtures/PROVENANCE.md`.
- Files compressed and committed via Git LFS to keep the repo clone-cost reasonable; CI fetches LFS objects.
- Sample `ComicInfo.xml`, `series.json`, `MetronInfo.xml`, `.cbl` files covering the full metadata surface, written by hand to exercise edge cases (missing fields, unicode, RTL manga, multi-arc).
- `fixtures/adversarial/` — synthetic malicious archives (§16.1) generated by a build script; these are tiny and committed directly (not LFS).
- A seed script that creates a default user, library, sample collections, and reading lists.
- **Fixtures must be checked into the repo (or LFS) so any clone can produce a populated environment without external downloads.**

### 13.5 Hot reload
- **Server:** `cargo watch -x run` (or `bacon`). Restart on Rust source change, ~3-5s rebuild typical.
- **Web:** `pnpm dev` (Next.js Turbopack). Sub-second HMR.
- **OpenAPI types:** a watch task regenerates `web/src/api/types.ts` when the spec changes.

### 13.6 Justfile commands (canonical)
```
just                      # list all commands
just bootstrap            # one-time setup
just dev                  # run server + web with hot reload
just dev-services-up      # start postgres/redis
just dev-services-down    # stop them, keep data
just dev-services-reset   # nuke postgres data
just test                 # run all tests (rust + js)
just test-rust            # cargo test
just test-web             # pnpm test
just test-e2e             # Playwright against running dev stack
just lint                 # clippy + eslint + prettier
just fmt                  # rustfmt + prettier
just migrate              # apply migrations
just migrate-new <name>   # generate new migration file
just seed                 # load fixtures
just openapi              # regenerate OpenAPI spec from Rust source
just docker-build         # build prod image locally
just docker-test          # run prod image against test fixtures (smoke test)
```

### 13.7 IDE setup
- VS Code recommended config in `.vscode/`: rust-analyzer, ESLint, Prettier, Tailwind IntelliSense.
- `recommended-extensions` list checked in.

### 13.8 Pre-commit hooks
- **Advisory mode through Phase 6.** `pre-commit` runs `cargo fmt --check`, `clippy`, `prettier --check`, `eslint`, and `oasdiff` against staged files; failures print warnings but do not block the commit. Rationale: rapid iteration during early phases; CI is the enforcement point.
- **Strict mode flips on at v1.0 GA.** Once iteration cadence slows post-launch, the same hooks block the commit on failure. The flip is a one-line change in `.pre-commit-config.yaml` (`fail_fast: true`) and a CONTRIBUTING.md note.

---

## 14. Repository Layout

Single monorepo. Cargo workspace + pnpm workspace coexist.

```
comic-reader/
├── Cargo.toml                      # workspace root
├── rust-toolchain.toml
├── crates/
│   ├── server/                     # Axum binary
│   │   ├── src/
│   │   │   ├── main.rs
│   │   │   ├── api/                # HTTP handlers (one file per resource)
│   │   │   ├── auth/
│   │   │   ├── library/            # scanner, parsers
│   │   │   ├── search/
│   │   │   ├── sync/               # Automerge integration
│   │   │   ├── opds/
│   │   │   └── ws/
│   │   └── tests/                  # integration tests
│   ├── entity/                     # SeaORM entity definitions
│   ├── migration/                  # SeaORM migrations
│   ├── parsers/                    # ComicInfo / series.json / MetronInfo / CBL parsers
│   ├── archive/                    # CBZ/CBR/CB7 readers
│   └── shared/                     # types shared between crates
├── web/                            # Next.js app
│   ├── package.json
│   ├── app/                        # App Router pages
│   ├── components/
│   ├── lib/
│   │   ├── api/                    # generated openapi-fetch client
│   │   ├── stores/                 # Zustand stores
│   │   └── reader/                 # reader-specific code
│   └── tests/
├── fixtures/                       # sample data for dev/test
│   ├── library/
│   ├── cbls/
│   └── seed.sql
├── docs/
│   ├── install/                    # user-facing install guides
│   ├── dev/                        # contributor docs
│   └── architecture/               # ADRs go here
├── compose.dev.yml
├── compose.prod.yml
├── Dockerfile
├── justfile
├── .env.example
├── .github/workflows/              # CI
├── CONTRIBUTING.md                 # raw-SQL escape hatch policy lives here
├── README.md
└── SPEC.md                         # this document
```

**iOS and Android directories** (`ios/`, `android/`) will be added at the top level when those phases begin. Not present in early phases.

---

## 15. Conventions

### 15.1 Naming
- **Rust:** `snake_case` everywhere (standard).
- **TypeScript/JS:** `camelCase` for variables/functions, `PascalCase` for components/types.
- **Database:** `snake_case` table and column names.
- **API JSON:** `snake_case` field names. The web client converts via `openapi-typescript`'s preserved naming; never use a transform layer that hides the wire format.
- **URLs:** `kebab-case` paths, plural resources (`/reading-lists`, `/series/{id}/issues`).

### 15.2 IDs
- All entities have UUID v7 primary keys (time-ordered, indexable).
- Issue stable ID is BLAKE3 hash (not UUID) — see §5.1.
- IDs are strings on the wire, always. No bigint exposure.

### 15.3 Error handling (server)
- Domain errors as typed enums per module. Top-level `ApiError` converts these to HTTP responses with consistent JSON:
  ```json
  { "error": { "code": "issue.not_found", "message": "...", "details": {...} } }
  ```
- Never expose internal error details (DB errors, file paths) to clients in prod.
- All errors logged with trace ID before being converted.

### 15.4 Logging
- `tracing` crate with structured fields. Never `println!` outside of tests.
- Levels: `error` (failed user action), `warn` (degraded state), `info` (lifecycle events), `debug` (per-request detail), `trace` (high volume).
- PII discipline: never log emails, paths inside user library, or full request bodies above `debug`.

### 15.5 Configuration validation
- All config validated on startup. Server refuses to start with invalid/missing required config and prints a clear message.
- No silent fallbacks for security-sensitive values (OIDC secret, etc.).

### 15.6 API design
- REST style, not RPC. Resources, plural names, standard verbs.
- Pagination: cursor-based (`?cursor=...&limit=...`) for lists that can grow unboundedly. Offset-based only for small bounded lists (collections, libraries).
- Consistent response envelope for lists:
  ```json
  { "items": [...], "next_cursor": "...", "total": 1234 }
  ```
- All timestamps RFC 3339 UTC.

### 15.7 Frontend conventions
- Server Components by default. `'use client'` only when interactivity requires it.
- **Server Actions are disabled** in `next.config.js` — all client→server traffic goes through the Rust API via `openapi-fetch` to keep the auth path single (§17.3).
- **Edge runtime is not used.** All Next.js routes run in Node.
- One component per file. Co-locate component-specific helpers and types.
- Tailwind classes ordered via `prettier-plugin-tailwindcss`.
- No global state except via Zustand stores; no React Context for app-wide data (TanStack Query is the cache).
- Routing carries a locale prefix (`/{locale}/...`) from day one even though only `en` ships at v1.0.

---

## 16. Testing Strategy

### 16.1 Server (Rust)
- **Unit tests** in each crate, colocated with source (`#[cfg(test)] mod tests`).
- **Integration tests** in `crates/server/tests/`:
  - Spin up real Postgres + Redis via `testcontainers-rs`. No mocking the DB.
  - HTTP assertions via `reqwest` against the running app.
  - Snapshot tests for OpenAPI spec (`insta` crate) — regenerated spec must match committed snapshot or CI fails.
  - **Breaking-change check:** `oasdiff` runs in CI comparing the PR's spec to `main`'s. Removed endpoints, removed fields, type changes, or weakened constraints fail the build unless the PR carries a `breaking-change` label and updates `CHANGELOG.md`.
- **Parser tests** in `crates/parsers/`: golden fixture files in / expected JSON out.
- **Adversarial-input fixtures** in `fixtures/adversarial/`:
  - 42 KB → 4 GiB nested ZIP bomb (synthesized at build time).
  - Zip-slip entry name (`../../etc/passwd`).
  - 60 000-entry ZIP (above max-entries cap).
  - 200:1 over-ratio entry.
  - Encrypted ZIP, encrypted RAR.
  - SVG entry inside CBZ.
  - Unicode-confusable filename inside CBZ.
  - XML files with DOCTYPE / external entity references (CBL, ComicInfo, MetronInfo).
  - 512 KiB `series.json` (above 256 KiB cap).
  - JPEG with malformed EXIF, AVIF with bad obu structure, JXL with invalid container.
  - Each fixture has an assertion: parser rejects with the correct typed error and never panics, allocates, or hangs beyond the §4.1.1 caps.
- **Property-based tests** for filename inference, page ordering, and CBL matching (via `proptest`).
- **Security middleware tests:** assert that responses on `/`, `/issues/.../pages/0`, `/api/me`, `/opds/v1`, and `/healthz` carry the correct headers (CSP, COOP, COEP, CORP, HSTS, Referrer-Policy, Permissions-Policy, X-Content-Type-Options).

### 16.2 Web (TypeScript)
- **Component tests** via Vitest + Testing Library.
- **Integration tests** via Playwright running against the dev stack (`just test-e2e`).
- **Visual regression** via Playwright screenshots for the reader and key list views. Run on PR; failures shown as image diffs.

### 16.3 End-to-end
- One Playwright suite that exercises critical paths: login → library → open issue → read pages → progress syncs → log into second browser → progress restored.
- Run against `compose.prod.yml` in CI before image publish (the "smoke test" — see §13.6 `just docker-test`).

### 16.4 Performance regression
- Bench suite for hot paths: page byte fetch, library list query, search query.
- Run in CI on PR. Fail if regressed >10% vs main baseline.
- `criterion` crate for Rust benches.
- Bundle-size regressions caught by `@next/bundle-analyzer` size-limit check; reader route fails CI if it exceeds the §18.1 budget.

### 16.5 Load tests by phase
- **End of Phase 2 (soak):** k6 script — 10 concurrent readers turn pages for 1 hour against `compose.prod.yml`. Assertions: no FD leak (`/metrics` `comic_zip_lru_open_fds` ≤ capacity), RSS does not grow > 10 % from start, no 5xx responses.
- **End of Phase 4 (sync stress):** 100 simulated clients connect/disconnect with offline edits via a Rust harness exercising the Automerge sync protocol. Assertions: convergence (all replicas equal after final sync), per-doc size stays under 16 MiB, no panics.
- **Phase 6 (full):** the four scenarios in §18.3 plus a 1k-issue scan benchmark.

### 16.6 Coverage targets
- Not enforced as a CI gate (counterproductive metric). Tracked for visibility only.
- Required: every parser, every search query type, every API endpoint has at least one test.

### 16.7 Accessibility tests
- `axe-core` automated audit runs in CI from Phase 2 onward via Playwright. Reader page, library grid, series detail, issue detail, search results pages all asserted clean of WCAG 2.2 AA violations.
- A focused manual audit milestone at Phase 3.5 (NVDA + VoiceOver walk-through, keyboard-only, reduced-motion).

---

## 17. Security

A one-page **threat model** lives at `docs/architecture/threat-model.md` (Phase 0 deliverable). It enumerates trust boundaries (reverse proxy, OIDC issuer, library mount, `/data` mount, Postgres, Redis, browser, mobile, OPDS readers) and per-component STRIDE notes.

### 17.1 Authentication
- Two modes via `COMIC_AUTH_MODE` (§3.1, §12.3): `oidc`, `local`, `both`.
- **OIDC:** PKCE (`S256`) for the auth code flow. JWKS fetched from issuer with TTL = min(issuer-specified, 1 h); JWKS refreshed on `kid` mismatch with exponential backoff. `aud`, `iss`, `nbf`, `exp` validated; ≤ 60 s clock skew tolerated. Algorithm pinned to the issuer's published `alg` set; `alg: none` and HS-when-RSA-expected rejected.
- **Local:** argon2id (`m=64 MiB, t=3, p=1`) password hashing; per-user salt; server-wide pepper at `/data/secrets/pepper`. Optional TOTP (RFC 6238, SHA-1 30 s window). `COMIC_LOCAL_REQUIRE_TOTP=true` makes TOTP enrollment mandatory after first login. Failed-login backoff: 1 s × consecutive-failure-count, capped at 30 s, per-user.
- **Local self-serve recovery (§17.1.1):** email-link password reset, gated on SMTP being configured. Disabled when SMTP is absent — the install guide tells admins to use `comic-reader admin-password-reset --email …` (CLI escape hatch) instead.
- **`email_verified`** missing → treated as **false** (§12.7).

### 17.1.1 Local self-serve account recovery
Available only when `COMIC_AUTH_MODE` includes `local` AND SMTP is configured (§12.3.1). Otherwise the recovery UI is hidden and the endpoints return 404.

**Email-verification on signup:**
1. Local registration creates the user row in `pending_verification` state. Server sends a signed verification link valid for 24 h.
2. User clicks the link → `GET /auth/local/verify-email?token=<…>` flips the user to `active`. Until then, login is rejected with `account.unverified`.
3. Re-send button on the login page (rate-limited per §17.7: 3 / hour / email).

**Password reset:**
1. `POST /auth/local/request-password-reset` with `{ email }`. Server **always returns 202 with no detail** (no account-existence oracle).
2. If the email matches a local user, server queues an email containing a signed reset link valid for 30 min, single-use.
3. User clicks → `POST /auth/local/reset-password` with `{ token, new_password, totp? }`. If the user has TOTP enrolled, the second factor is required to complete the reset (prevents email-account compromise alone from taking over the account).
4. Successful reset bumps `users.token_version` (revokes all existing sessions per §17.2) and writes an `auth.password_reset` audit-log entry.

**Token construction:**
- Verification token: HMAC-SHA256 over `(user_id, email, exp, kind='verify')` with a server-side key under `/data/secrets/email-token-key`. Stateless — no DB row.
- Reset token: same HMAC scheme with `kind='reset'`, **plus** a `password_reset_uses` row recording (`token_id`, `user_id`, `consumed_at`) for single-use enforcement. Row TTL = 1 h.

**Email content:**
- From address from `COMIC_SMTP_FROM`.
- Plain-text + HTML alternative; never includes the password, only the link.
- Link uses `COMIC_PUBLIC_URL` as the base — no Host-header reflection.
- Optional DKIM signing if `COMIC_SMTP_DKIM_KEY_FILE` is set.

**Rate limits (additional bucket on top of §17.7):**

| Bucket | Per-IP | Per-email | Burst |
|---|---|---|---|
| `POST /auth/local/request-password-reset` | 5 / hour | 3 / hour | 5 / 5 |
| `POST /auth/local/resend-verification` | 5 / hour | 3 / hour | 5 / 5 |
| `POST /auth/local/reset-password` | 10 / hour | — | 10 |

### 17.2 Sessions, JWT, and revocation
- Access JWT TTL = 15 min (`COMIC_JWT_ACCESS_TTL`). Refresh token TTL = 30 days, **rotated on every use**. Refresh token hashes stored in `auth_sessions(id, user_id, refresh_token_hash, last_used_at, ua, ip, revoked_at)`.
- `users.token_version` column embedded in JWT claims; bumped on password reset, admin-revoke, or scope change. Mismatched version on any request → 401, force re-auth.
- `POST /auth/logout` revokes the current refresh row and bumps `token_version`. Admin "log out all sessions" available at `POST /admin/users/{id}/revoke-sessions`.
- **Web app**: session JWT lives in an `httpOnly`, `Secure`, `SameSite=Lax` cookie (`__Host-comic_session`). Refresh token lives in a separate `__Secure-comic_refresh` cookie (Path=`/auth/refresh`; the `__Host-` prefix is incompatible with a non-`/` Path so `__Secure-` is used). CSRF token lives in `__Host-comic_csrf` (not HttpOnly so JS can read it for the double-submit header). Cookie domain is host-only (no wildcard).
- **Mobile / API / OPDS-Bearer**: JWT in `Authorization: Bearer …`.

### 17.3 CSRF
- Web auth via cookie → CSRF protection required.
- On `GET /auth/me`, server returns a CSRF token in the JSON body and as a `__Host-comic_csrf` non-httpOnly cookie. Token is per-session, rotated on auth and on logout.
- Every unsafe verb (POST/PUT/PATCH/DELETE) requires `X-CSRF-Token` header matching the cookie value (double-submit pattern). Mismatch → 403.
- Bearer-authenticated requests skip the CSRF check (no ambient credential to forge).
- WebSocket auth uses single-use tickets (§9.6) — not vulnerable to CSWSH because the upgrade carries no cookie credential.

### 17.4 Content Security Policy and security headers
Sent on every HTML response. Hashes/nonces injected by Next.js middleware:

```
default-src 'self';
script-src 'self' 'strict-dynamic' 'nonce-{NONCE}';
style-src 'self' 'nonce-{NONCE}';
img-src 'self' data: blob:;
font-src 'self';
connect-src 'self' {OIDC_ISSUER_ORIGIN} wss://{HOST};
frame-ancestors 'none';
form-action 'self' {OIDC_ISSUER_ORIGIN};
base-uri 'none';
object-src 'none';
worker-src 'self' blob:;
manifest-src 'self';
require-trusted-types-for 'script';
upgrade-insecure-requests;
report-to comic-csp;
```

Companion headers (always):
- `Strict-Transport-Security: max-age=63072000; includeSubDomains` (when public URL is HTTPS).
- `Cross-Origin-Opener-Policy: same-origin`
- `Cross-Origin-Embedder-Policy: credentialless` (enables SharedArrayBuffer for the worker decode pipeline).
- `Cross-Origin-Resource-Policy: same-origin` on page-byte and thumb endpoints.
- `Referrer-Policy: strict-origin-when-cross-origin`
- `Permissions-Policy: camera=(), microphone=(), geolocation=(), usb=(), bluetooth=(), payment=()`
- `X-Content-Type-Options: nosniff`
- `X-Frame-Options: DENY` (legacy belt-and-braces alongside `frame-ancestors`).

CSP violations posted to `/csp-report` (rate-limited, 100/min/IP) and surfaced in `/metrics`.

### 17.5 Page-byte and thumbnail safety
- Server sniffs the **first 16 bytes** of every page entry; trusts magic bytes, not the archive entry's filename.
- `Content-Type` is set from the sniff allowlist (`image/jpeg`, `image/png`, `image/webp`, `image/avif`, `image/gif`, `image/jxl`). Anything else → 415.
- `Content-Disposition: inline; filename="page-{n}.{ext}"` derived from sniff.
- SVG entries inside archives are rejected outright.
- Thumbnails on disk are **content-addressed**: `/data/thumbs/<blake3-of-source-bytes>.webp`. Identical pages across issues dedupe automatically; URLs are not user-guessable in a way that bypasses ACLs.
- `/data` files are served by the Rust binary, never by Next.js — auth and ACL checks happen first.

### 17.6 Archive defenses
See §4.1.1 for the full set of caps and subprocess limits. In short: zip-slip rejection at entry-name validation, hard caps on entry count / total bytes / single-entry bytes / compression ratio / nesting depth, `prlimit`-bounded subprocesses, no shell invocation, encrypted archives surfaced rather than retried.

### 17.7 Rate limiting
Token-bucket limits enforced via `tower-governor`, Redis-backed when Redis is configured (otherwise in-memory). "User" = JWT subject or app-password ID. "IP" = first untrusted hop in `X-Forwarded-For`, honoring `COMIC_TRUSTED_PROXIES`.

| Bucket | Per-IP | Per-user | Burst |
|---|---|---|---|
| `GET /issues/.../pages/.*` | 60 req/s | 300 req/s | 120 / 600 |
| `GET /issues/.../pages/.*/thumb` | 200 req/s | 1000 req/s | 400 / 2000 |
| `GET /search` | 5 req/s | 20 req/s | 10 / 40 |
| `GET /search/autocomplete` | 30 req/s | 60 req/s | 60 / 120 |
| `POST /libraries/{id}/scan` | 1 / 5 min | 1 / 5 min | — |
| `POST /auth/oidc/callback`, `POST /auth/local/login` | 5 / min | — | 10 |
| OPDS `*` (Basic Auth) | 30 req/s | 60 req/s | 60 / 120 |
| Failed-auth (any) | 10 / min / IP | — | — |
| `POST /csp-report` | 100 / min | — | — |
| `POST /ws/ticket` | 30 / s | — | — |

Bucket exhaustion → 429 with `Retry-After`. Failed-auth bucket triggers an extra 15-min OPDS lockout per IP.

### 17.8 Audit logging
All admin and security-sensitive actions written to `audit_log` (§5.9). Reads admin-only; never returned in non-admin responses.

### 17.9 Secrets management
- Server-side keys (URL-signing, app-password pepper) generated on first start under `/data/secrets/`, mode `0600`, owned by the app user. Backed up alongside Postgres.
- All secret-bearing env vars accept a `_FILE` suffix (Docker secrets compatibility).
- SIGHUP reloads OIDC config and signing keys without restart.
- Postgres/Redis credentials never logged at any level. Logs scrubbed via `tracing` field redaction.
- `cargo deny` and `cargo audit` run in CI; `pnpm audit --audit-level=high` runs in CI.

### 17.10 Data subject rights (self-served)
- `GET /me/export` (re-auth required: re-enter password / OIDC popup) returns a JSON archive containing the user's progress, bookmarks, ratings, reviews, collections, reading lists, app passwords (hashes redacted), audit-log rows where the user is the actor.
- `DELETE /me` cascade-deletes user-owned content with explicit options at request time: `reviews_action: 'delete' | 'anonymize'`. Admin notified via audit log + system notification. 7-day soft-delete window (recoverable by admin); after 7 days, hard-deleted.

### 17.11 SSRF defense (forward-looking)
Outbound HTTP from the server (when ComicVine/Metron enrichment lands in Phase 7+) routes through a single `crates/server/http_outbound::SafeClient`:
- Resolves DNS once, validates the resolved IP against a denylist (RFC 1918, link-local, loopback, IPv6 ULA, multicast), then connects to that IP.
- Pins TLS roots (system CA bundle); validates hostname.
- Per-host concurrency cap (4) and request timeout (10 s).
- Body size cap (8 MiB) and content-type allowlist on responses.

### 17.12 Other
- File-path traversal guard on the scanner: canonicalize and confirm `library_root.contains(resolved_path)`. Symlinks inside `/library` are followed only if their target resolves inside `/library`.
- Optional age-rating gating per user via `library_user_access.age_rating_max` (§5.1.1).
- XML parsers (CBL, ComicInfo, MetronInfo) configured with **DOCTYPE rejected, external entities disabled** (`quick-xml` with restrictive settings); XXE-safe.
- JSON inputs (`series.json`) capped at 256 KiB pre-parse; `comic_image` fields are never auto-fetched in v1.
- WebSocket message bounds in §9.4.

---

## 18. Performance Targets

### 18.1 Targets (p95 unless stated otherwise)
- **First page byte (warm OS page cache, ZIP central dir in §7.2.1 LRU):** p95 < 200 ms LAN, < 800 ms WAN.
- **First page byte (cold cache, NVMe-backed library):** p95 < 500 ms LAN. Spinning-rust libraries are explicitly out-of-scope for this target.
- **First page byte tail:** **p99 < 1 s** under any condition short of disk failure.
- **Page decode + paint (web):** < 100 ms on M1-class, < 250 ms on midrange Android. Decode pipeline = `createImageBitmap` in a Web Worker (§7.2).
- **Library list (1000 series):** < 300 ms server-side response.
- **Search query (unified):** < 150 ms server-side for 95% of queries.
- **Autocomplete:** < 50 ms server-side for 95% of queries.
- **Full library scan (Phase A — metadata + hash):** > 1000 issues/min on NVMe.
- **Thumbnail generation (Phase B — async background):** > 500 issues/min/core; never blocks Phase A.
- **Server memory:** idle < 150 MB; under 1 GB serving 50 concurrent readers.
- **Reader bundle size:** ≤ 150 KB JS gzipped on the `/read/[issue]` route. Excluded from this route's chunk graph: Tiptap, dnd-kit, framer-motion (§3.1).

### 18.2 Measurement methodology
- All server-side targets measured via Prometheus histograms exported by Axum middleware.
- Web targets measured via real-user metrics (RUM) sent to the server's `/metrics-rum` endpoint and aggregated in Prometheus. No third-party RUM service.
- Bench suite (`criterion` for Rust, `lighthouse-ci` for web) runs in CI on PR; results compared against `main` baseline. PRs that regress > 10% require review approval.
- Bundle size regressions caught by `@next/bundle-analyzer` size-limit check in CI.

### 18.3 Load test scenarios
Documented in `docs/dev/load-testing.md` (added in Phase 6):
- 50 concurrent readers turning pages
- 1 active scan + 10 concurrent readers
- 1000-series library list pagination
- Search query mix (autocomplete + full search) at 100 QPS

---

## 19. Roadmap

Phases are sized to be independently shippable. Each phase ends with a working, tested, deployable build. The server and web app are co-developed through Phase 6; mobile is deferred to Phase 8+.

Each phase entry calls out **forward-compat considerations** — things to design now even though they're not implemented until later — so we don't paint ourselves into a corner.

### Phase 0 — Foundation (server + web skeleton, no features)
- Cargo workspace with crate layout (§14).
- pnpm workspace with Next.js scaffold.
- Postgres extensions enabled day one (`pg_trgm`, `unaccent`, `fuzzystrmatch`).
- SeaORM connected, first migration runs (includes `users`, `auth_sessions`, `library_user_access`, `audit_log`).
- **Auth:** both modes wired (OIDC + local). Cookie session + CSRF middleware. Refresh-token rotation. JWT key rotation via JWKS. WebSocket ticket endpoint (no actual WS yet).
- **Security middleware:** CSP (§17.4), companion headers, rate-limit middleware (`tower-governor` skeleton with the §17.7 table).
- `/healthz`, `/readyz`, `/metrics`, structured logging, OpenTelemetry default-on with stdout exporter.
- CI: build, test, lint, format, container build, `cargo deny`, `cargo audit`, `pnpm audit`, `oasdiff` skeleton, license check.
- `compose.dev.yml` and `compose.prod.yml` both work; `compose.prod.yml` includes Redis (apalis prerequisite).
- Justfile with all canonical commands (§13.6).
- Sample fixture data committed (including `fixtures/adversarial/` skeleton).
- Phase 0 deliverable docs: `docs/architecture/threat-model.md`, `docs/architecture/auth-model.md`, `docs/architecture/csp.md`, `docs/architecture/rate-limits.md`.
- **Forward-compat:** OpenAPI generation pipeline in place; `utoipa` annotations on stub handlers. Web's API client codegen wired up. `next-intl` with `/{locale}/...` routing scaffolded (`en` only at this point) so i18n retrofit isn't painful.
- **Done when:** `git clone && just bootstrap && just dev` works; you can log in via OIDC *and* via local username/password and see "hello world." First user becomes admin (§12.8). All security headers present in the response. CSP report endpoint accepts a synthetic violation.

### Phase 1a — Library scan & basic browse (CBZ-only foundation)
- Recursive library scanner with `notify` watch (resumable via `scan_runs` checkpoint table).
- **Two-phase scan model** (§B3 of review): Phase A walks FS + hashes + parses metadata + inserts Issue rows; Phase B (apalis-queued) generates thumbnails asynchronously. Browser shows placeholder until ready; WS event delivers update.
- Archive reader: **CBZ only** for this phase, with full §4.1.1 archive-defense limits in place.
- Parser: **ComicInfo.xml + filename inference fallback** only. (series.json and MetronInfo land in 1b.)
- Issue dedupe by content hash (BLAKE3); `dedupe_by_content=true` is the default (§5.1.2).
- Encrypted CBZ detection (§4.6).
- Schema: Series, Issue, Library, User, ProgressRecord (placeholder), `library_user_access` (schema only — UI in Phase 5), `audit_log`, ComicVine/Metron ID columns.
- API endpoints: list libraries, list series, list issues, get issue metadata.
- Web: library grid (virtualized), series page, issue detail page. No reader yet.
- **Forward-compat:** Page list as JSONB on Issue. Search corpus design (§6.3) implemented for series + issue.
- **Done when:** scan a CBZ-only fixtures library, browse it on the web, see correct ComicInfo metadata. Adversarial-fixture suite (§16.1) passes — every malicious archive rejected with the expected typed error.

### Phase 1b — Format coverage, async thumbnails, search foundation
- Archive readers: CBR (via `unrar` subprocess with §4.1.1 limits), CB7 (`sevenz-rust`), CBT (stdlib `tar`), folder-of-images, EPUB with §4.5 comic-style predicate.
- Additional parsers: series.json (Mylar3), MetronInfo.xml.
- Thumbnail pipeline runs as full apalis worker (background).
- API endpoints: get page thumbnail.
- Per-context search (`/series?q=`, `/series/{id}/issues?q=`) using `tsvector` on series name + issue title.
- `library_user_access` predicate baked into all read queries (defaulting to "everyone has access" until UI lands).
- **Done when:** Phase 1 spec as originally written is delivered. All 4 archive types supported with adversarial-fixture coverage. Full ComicInfo + series.json + MetronInfo metadata visible.

### Phase 2 — Reader v1 + basic OPDS
- Page byte streaming endpoint (`Range` request support; correct 416 / `Accept-Ranges` semantics; content-type sniffed per §17.5).
- ZIP central-directory LRU (§7.2.1).
- JXL → AVIF transcode pipeline (apalis-queued during scan).
- Web reader: single-page mode, LTR, keyboard nav, tap zones, fit modes (width/height/original).
- Page preload (N+1, N+2) using `createImageBitmap` in a Web Worker (§7.2).
- Reader-local state (Zustand): current page, zoom, fit mode, chrome visibility.
- Progress recorded to server (no Automerge yet; simple POST per page turn — will be replaced in Phase 4).
- Reader bundle budget enforcement via `@next/bundle-analyzer` in CI; framer-motion / Tiptap / dnd-kit excluded from `/read/[issue]`.
- **Basic OPDS 1.2 catalog** (root, series, recent, search) — JWT-only auth; no app passwords, no PSE yet. Lets external readers connect early.
- `axe-core` audit added to CI for all reader and library views.
- **End-of-phase soak test** (§16.5).
- **Forward-compat:** progress write API shaped so it can be backed by Automerge in Phase 4 without changing client behavior — clients write to a local store that the network layer drains, swap drain implementation later.
- **Done when:** open an issue, page through it, close, reopen, resume on the same page. Range request edge cases pass test. ZIP-LRU FD count visible in `/metrics`. KOReader successfully browses the OPDS feed (no PSE yet).

### Phase 3 — Reader v2 (modes + polish)
- Double-page mode with auto-detection from ComicInfo `DoublePage` flag and aspect ratio.
- RTL reading direction (auto from `Manga=YesAndRightToLeft`).
- Vertical webtoon / continuous-scroll mode.
- Custom gestures via `@use-gesture/react`; CSS transitions for page-turn (no framer-motion in reader bundle).
- Color modes (normal, sepia, OLED black).
- Per-series remembered settings (stored locally; will sync in Phase 4); per-user **default reading direction** preference.
- Mini-map / page strip overlay.
- `prefers-reduced-motion` honored.
- **Done when:** reader feels polished on desktop and mobile web; passes manual UX review.

### Phase 3.5 — Accessibility audit (1 week)
- Manual NVDA + VoiceOver walk-through of every reader mode and key list view.
- Keyboard-only walk-through (no mouse / touch) covering library → series → reader → search.
- Reduced-motion verification across all transitions.
- Color-contrast verification on sepia and OLED-black themes.
- Findings filed as P0/P1/P2; P0/P1 fixed before exiting the milestone.
- `axe-core` CI assertion expanded to cover all routes added in Phases 2–3.
- **Done when:** WCAG 2.2 AA self-attestation written to `docs/architecture/accessibility.md` with the audit log.

### Phase 4 — Sync (Automerge)
- Automerge document model: per-user docs for progress, reader settings, future-bookmarks (§9.2).
- Compaction policy in place day one (§9.3).
- WebSocket sync protocol with one-shot ticket auth (§9.6); HTTP polling fallback.
- WS DoS bounds in place (§9.4).
- IndexedDB-backed local replicas in web client.
- Custom merge rule: `max(page)` for last-page-read.
- **Phase 2 → Phase 4 migration milestone** (§9.7): backfill existing `progress_records` into Automerge docs on first connect; bump API version; old clients receive 410 Gone.
- PWA manifest and service worker so the web app works offline. Service worker scope: shell + recently-read issue metadata. **Page bytes are NOT cached by default** (license / storage); user can explicitly "save for offline" per issue.
- End-of-phase sync stress test (§16.5).
- **Forward-compat:** Automerge document schemas explicitly version themselves so future fields (annotations, shared collections) can be added without breaking older clients.
- **Done when:** read on Browser A, walk away, open Browser B, progress is current. Disconnect, read more, reconnect, merges cleanly. Backfill from Phase 2 verified by integration test.

### Phase 5 — Lists, arcs, reviews + library access UI
- Collections (series-level and issue-level), drag-reorder via `dnd-kit`.
- Reading lists with strict order.
- CBL import wizard with mapping screen (§5.3); CBL export. XML parser configured XXE-safe (§17.12).
- Story arcs auto-built from ComicInfo, manually editable, sticky-flag override.
- Reviews + ratings (per-issue and per-series); Tiptap editor for review body. Spoiler-blur a11y-correct (§7.4, §17.4).
- Admin moderation UI (delete with reason; logged in `audit_log`).
- **Library access management UI** (§5.1.1) — admin can grant per-user library access and `age_rating_max`. Schema has been live since Phase 1; this exposes it.
- **Komga / Kavita progress import** endpoint (`POST /admin/import/progress`) with documented JSON format.
- Search corpus extended to reviews, collections, reading lists, bookmarks.
- Collections/lists sync via Automerge (extends Phase 4); shared-collection semantics per §9.5.
- **Forward-compat:** reading list entries support `volume_override` and `number_override` columns now, so future CBL refinements don't require migration.
- **Done when:** import a real-world CBL of a major event (Civil War, House of M), browse it, share progress across devices. A second user with restricted library access sees only the libraries they're granted, with `age_rating_max` enforced.

### Phase 6 — Search v2 + OPDS v2 + v1.0 GA
- Unified `/search` endpoint (§6.4) with `UNION ALL` typed sub-queries; `websearch_to_tsquery` + 2 s `statement_timeout` (§6.4 hardening).
- Facets, ranking boosts, recency-weighted personalization.
- `ts_headline` highlighting.
- "Did you mean" via `levenshtein` against curated dictionary.
- OPDS 2.0 (JSON) catalog (OPDS 1.2 already exists from Phase 2).
- OPDS-PSE for compatible mobile readers; signed URLs (§8.3).
- OpenSearch description doc.
- App passwords for OPDS auth (§8.1, §8.2 — read / read+progress scopes).
- Full load-test scenarios from §18.3 + adversarial query mix.
- **Self-service data rights:** `GET /me/export` and `DELETE /me` shipped (§17.10).
- **Forward-compat:** search response shape stable enough to back with Meilisearch later if needed.
- **Done when:** an external OPDS reader (Chunky on iPad, KOReader on Kobo) can connect and stream pages with PSE. The search experience feels good on a 10k-issue library.

### v1.0 GA gate (post-Phase 6)
- Phases 0–6 = complete server+web product. Tag `v1.0.0` on merge.
- Documented upgrade path forward (Phase 7+ are post-1.0 enhancements).
- Public install docs reference `comic-reader:1` and `comic-reader:1.0` tags (§12.2).

### Phase 7 — Relationships (post-1.0)
- `series_relationships` table with directed/typed edges and inverse-pair auto-creation.
- Recursive traversal queries (raw SQL via SeaORM connection); CTE depth capped at 6.
- Suggestion engine: apalis job after each scan generates candidates from ComicInfo + filename heuristics. Per-scan candidate cap = 1000 to bound runtime.
- Admin review UI: pending suggestions grouped by confidence, bulk-accept high-confidence, individual review for medium/low.
- Append-only audit trail on suggestions (uses unified `audit_log`).
- **Done when:** point at a Marvel-heavy library, get sensible suggestions for crossovers and series continuations, accept/reject from a clean UI.

### Phase 8 — iOS app (deferred until after Phase 7 is stable)
- SwiftUI + SwiftData. Targets iOS 17+.
- OIDC login via ASWebAuthenticationSession.
- Library browse, series detail, issue detail.
- Reader (single page, double page, RTL, webtoon — feature parity with web).
- Background download for offline reading.
- Automerge progress sync via Swift bindings.
- Files app integration for sideload.
- iPad split-view library/reader.
- TestFlight distribution; App Store later if appropriate.
- **Done when:** can replace a Kavita+Panels setup on iOS with this stack.

### Phase 9 — Android (only if motivated)
- Kotlin Multiplatform skeleton, Jetpack Compose, Material 3.
- Same feature scope as iOS Phase 8.
- Foreground service for downloads. Storage Access Framework for sideload.
- This phase is **explicitly aspirational**. May never ship.

### Phase 10 — Polish & long-tail features
- Annotations (database-backed, sync via Automerge).
- Search analytics dashboard.
- Multi-language search tuning (especially Japanese).
- Performance optimization based on real-world load.
- Anything from §21 Backlog that's earned its way in.

### Phasing notes
- Phases 0–6 are server+web only and constitute v1.0. Mobile contributors do not need to touch them; the API contract is the integration point.
- Phases 1a–3 are the "minimum viable comic reader." Stop here and you have a usable product; everything beyond is enhancement. Basic OPDS in Phase 2 means external readers work from that point on.
- Phase 4 (Sync) was moved before Phase 5 (Lists) because designing Automerge documents requires knowing roughly what they'll hold; getting progress sync working first proves the pattern, then collections/lists fit in cleanly.
- Phase 1 was split (1a / 1b) because the original Phase 1 was overstuffed (scanner + 4 archive readers + 3 metadata parsers + dedupe + thumbnails + 5 endpoints + 3 web pages + search). The split lets 1a ship with CBZ-only — still useful — and de-risks the most complex archive readers (CBR / CB7) into 1b.
- Phase 3.5 (Accessibility audit) is a hard gate, not an aspiration. Failure to meet WCAG 2.2 AA blocks Phase 4.

---

## 20. Decisions & Open Questions

### 20.1 Resolved decisions
- **Build priority** — Server + web are v1.0 (Phases 0–6). iOS is post-1.0 (Phase 8). Android may never ship. See §1.1.
- **PDF input** — Explicitly out of scope (§1.2). Scanner ignores `.pdf`.
- **Auth model** — `COMIC_AUTH_MODE=oidc|local|both` (§3.1, §17.1). Cookie session + double-submit CSRF for web; Bearer for mobile/API/OPDS; one-shot ticket for WebSocket (§9.6, §17.3).
- **JWT lifetime** — 15 min access, 30 d rotating refresh; `users.token_version` for revocation (§17.2).
- **App-password scopes** — Two presets: `read` and `read+progress` (§8.2).
- **OPDS-PSE signed URLs** — HMAC-signed, 24 h default expiry (§8.3).
- **Background jobs** — `apalis` (Redis-backed). Redis effectively required in prod; in-process fallback only for `just dev` (§3.1).
- **Library access control** — `library_user_access` schema lands Phase 1; admin UI lands Phase 5. Schema present means queries are correctly scoped from day one (§5.1.1).
- **Default admin bootstrap** — First successful login on a fresh database becomes admin; install docs require closing registration before opening the app to the network (§12.8).
- **Container process model** — Two processes (Rust + Next standalone) under `tini` PID 1; Next reverse-proxied via Rust (§12.1.2). Server Actions disabled.
- **Issue dedupe** — `dedupe_by_content=true` is the default (§5.1.2).
- **Image transcoding (one carve-out from §1.2 non-goals)** — JXL → AVIF/WebP server-side at scan time. Original preserved for OPDS download (§7.2).
- **WebCodecs** — Not used. `createImageBitmap` in a worker is sufficient (§7.2).
- **Reader bundle exclusions** — Tiptap, dnd-kit, framer-motion explicitly excluded from `/read/[issue]` (§3.1, §18.1). Page-turn animations use CSS only.
- **Accessibility commitment** — WCAG 2.2 AA, audit gate at Phase 3.5 (§7.4, §16.7, §19).
- **i18n** — `next-intl` with `/{locale}/...` routing scaffolded in Phase 0; only `en` shipped initially.
- **Data subject rights** — `GET /me/export` and `DELETE /me` shipped at v1.0 (§17.10).
- **Rate-limit table** — Concrete numbers in §17.7.
- **CSP and security headers** — Concrete policy in §17.4.
- **Audit log** — Unified `audit_log` table (§5.9).
- **OpenTelemetry** — Default-on with stdout exporter (§12.4).
- **OpenAPI breaking-change check** — `oasdiff` in CI; PRs need explicit `breaking-change` label (§16.1).
- **`COMIC_AUTH_MODE` precedence** — When `both`, the user picks at login screen; admin can disable either method via env at start.
- **RAR support** — host-installed `unrar` binary (provided by Docker base image). License documented in `LICENSE-THIRD-PARTY.md`.
- **RAR support (legacy entry, retained for historical context)** — host-installed `unrar` binary. No license bundling beyond `LICENSE-THIRD-PARTY.md` notice.
- **Search corpus tuning (§6.3)** — ComicInfo `<Notes>` is **not** indexed (community noise field); creator names stay at C-weight on issues. Field-to-weight table in §6.3 is final for v1.
- **Search analytics retention (§6.10)** — rolling **90 days**. Nightly apalis job prunes older rows.
- **Sample comics for fixtures (§13.4)** — public-domain Golden Age titles via Git LFS, with provenance recorded in `fixtures/PROVENANCE.md`.
- **Pre-commit hooks (§13.8)** — advisory through Phase 6, strict at v1.0 GA.
- **iOS minimum version (§11.1)** — **iOS 17**. Unlocks SwiftData, Observable macro, TipKit; older devices fall back to web reader.
- **Series completion semantics (§5.5.1)** — four states: `complete`, `caught_up`, `reading`, `unread`. `caught_up` distinguishes "all known issues read but series ongoing" from true `complete` (`series.status == 'ended'`). Default `series.status` is `continuing` when missing — never claims `complete` without explicit confirmation.
- **Reference Caddyfile (§12.1.1)** — committed inline; `docs/install/caddy.md` is a verbatim copy plus an nginx counterpart at `docs/install/nginx.md`.
- **Local self-serve account recovery (§17.1.1)** — email-link verification + password reset, gated on optional SMTP (§12.3.1). When SMTP is absent, recovery UI is hidden and admins use `comic-reader admin-password-reset` CLI. Reset always requires TOTP if enrolled (defends against email-account compromise).
- **Annotations** — deferred entirely. When added later, will be database-backed (no sidecar JSON files).
- **Kavita-compat adapter** — not building. Users on Kavita can use a one-shot migration tool if/when one exists.
- **ComicInfo.xml writes** — read-only forever. Server never modifies user files.
- **CRDT** — Automerge 2.x. See §3 and §9.2.
- **SQLite-lite mode** — not supporting. Postgres required for consistency, full-text search, recursive CTEs.
- **Reviews moderation** — author edit/delete; admin delete-only with reason logged. No community flagging. Spoiler tag is an optional self-serve flag, not enforced.
- **Review reactions / community signals** — deferred to backlog (§21).
- **CBL import** — multi-step wizard with explicit mapping screen. No ghost rows. See §5.3.
- **Series relationships** — user-confirmed only, but the system *suggests* candidates from ComicInfo fields (`AlternateSeries`, `SeriesGroup`), filename patterns, and metadata heuristics. Suggestions appear in a "Review proposed connections" admin screen with confidence scores; user accepts/rejects/edits before any edge is created. No edges ever created without user confirmation. See §5.7 for details.
- **Next.js deployment** — `output: 'standalone'` in Docker.
- **Reader bundle budget** — `/read/[issue]` route ≤ 150KB JS gzipped, enforced via `@next/bundle-analyzer` in CI. Lazy-load Tiptap, dnd-kit, framer-motion on routes that need them; never bundle them into the reader.
- **API client (web)** — `openapi-typescript` for types + `openapi-fetch` for runtime (~6KB), wrapped with TanStack Query manually.
- **OpenAPI spec source** — generated from Rust source via `utoipa` annotations. Spec file checked in and CI fails if it drifts from generated output.
- **Search backend** — Postgres-only (`tsvector` + `pg_trgm` + `unaccent` + `fuzzystrmatch`). Meilisearch is a deferred upgrade path; unified search API (§6.4) is designed so swap requires no client changes. See §6.
- **Raw-SQL escape hatch** — explicit allowlist of queries that bypass SeaORM entities, documented in `CONTRIBUTING.md`. Initial list: (a) library list page (paginated series + cover + unread count), (b) page byte fetch (no DB hit ideally), (c) progress sync delta, (d) recursive series-relationship traversal, (e) unified `/search` endpoint (UNION ALL across entity sub-queries with normalized scoring, §6.4), (f) `/search/autocomplete` (pg_trgm similarity ordering). Adding to the list requires PR review; benefit is keeping codebase style consistent.
- **No object storage in v1** — filesystem (`/data` volume) for thumbnails. S3/MinIO can be added later behind a storage abstraction.
- **Logical-modules-not-microservices** — the architecture diagram in §2 shows logical modules within a single Axum binary. Separate processes are a future option, not a v1 goal.

### 20.2 Open questions
*All known open questions are resolved as of v0.5. Use this section to capture new questions as they surface during implementation.*

---

## 21. Backlog (deferred but tracked)

Ideas explicitly considered and deferred. Don't lose them.

- **In-image translation** — OCR + MT pipeline to translate speech bubbles. Considered v0.1, removed because of complexity. Could return as a separate optional service.
- **Annotations** — bookmarks-with-text on specific pages, regions, or panels. Deferred to Phase 10. Database-backed when added.
- **Review reactions / community signals** — likes, "helpful" markers on reviews. Deferred indefinitely; probably doesn't fit a self-hosted model.
- **External metadata enrichment** — Comic Vine API or Metron API integration for richer series metadata and relationship suggestions. ID columns already exist (§5.1) so adding enrichment is a non-migration job. Held back due to API key management and rate limits. Outbound HTTP must route through `SafeClient` (§17.11).
- **Object storage backend (S3/MinIO)** — abstraction in place but not implemented. Add when filesystem becomes a bottleneck.
- **Meilisearch upgrade path** — documented in §6.1. Add when Postgres search hits limits.
- **`pgvector` for semantic search** — "find comics like Saga." Needs embedding pipeline. Phase 10+ experiment.
- **Kavita-compat adapter** — explicitly rejected (§20.1) but kept here for visibility. Note: `POST /admin/import/progress` (§10) is a documented JSON format that lets community write Kavita/Komga importers.
- **Mobile annotations / drawing** — Apple Pencil support on iPad in Phase 8. Stretch goal even within that phase.
- **Cast / external display support** — sketched in old §11.3, deferred.
- **Family multi-user UX** — partial: per-user progress is solved by Automerge sync (Phase 4); per-user library access + age-rating gate ships in Phase 1 schema / Phase 5 UI (§5.1.1).
- **Kavita/Komga migration tool** — one-shot importer that reads existing Kavita/Komga DBs. Useful but not core; `POST /admin/import/progress` shipped at v1.0 covers progress.
- **Guest / share links** (`/s/{token}`) — already in §10 API surface; UI lands at v1.1.
- **Reading stats / streaks** — "issues read this year," "longest streak," "most-read series." Cheap given progress data; v1.1 candidate.
- **Granular app-password scopes** — beyond `read` and `read+progress`, finer scopes (ratings-only, OPDS-only). Defer until users ask.
- **Per-collection Automerge docs** — true CRDT collaboration on shared collections. v1 uses owner-authoritative (§9.5).
- **Public-domain demo library** — bundled fixtures with PD comics for first-run "explore mode."
- **SSO via SAML** — broadens self-host audience beyond OIDC. No demand yet.
- **Slug-friendly URLs** — currently series and issue URLs carry the raw UUID / BLAKE3 hash (`/series/019deb7e-2fa7-7020-…`). The pragmatic upgrade is `/series/{slug}-{shortid}` (GitLab/Notion style — first 8 hex of the UUID gives uniqueness, slug is purely decorative). Resolver matches on the shortid suffix, so renames don't break old links and slug collisions across libraries are a non-issue. Issue stable IDs are content hashes, so `/issues/{slug}-{hashprefix}` works the same way. Candidate for v1.1; no schema change needed.

---

## 22. Glossary

- **ADR** — Architecture Decision Record. Markdown docs in `docs/architecture/` capturing why a non-obvious choice was made.
- **Automerge** — CRDT library used for sync. Documents merge automatically across replicas.
- **CBL** — Comic Book List. XML-based reading list format used by Mylar3 and similar tools.
- **CBR / CBZ / CB7 / CBT** — Comic book archives: RAR, ZIP, 7z, TAR respectively. CBZ is by far the most common.
- **ComicInfo.xml** — Per-issue metadata file inside an archive. Anansi Project schema is the standard.
- **CRDT** — Conflict-free Replicated Data Type. Data structure that merges deterministically from multiple replicas.
- **CTE** — Common Table Expression. SQL `WITH ...` clause; `RECURSIVE` variant powers graph traversal.
- **GIN** — Generalized Inverted Index. Postgres index type used for `tsvector` and `pg_trgm`.
- **JWT** — JSON Web Token. Used for session tokens after OIDC login.
- **LWW** — Last-Writer-Wins. Conflict resolution strategy where the most recent write supersedes earlier ones.
- **MetronInfo.xml** — Alternative per-issue metadata schema with richer creator/credit support.
- **Mylar3** — Comic management tool. Source of `series.json` schema.
- **OIDC** — OpenID Connect. Authentication protocol used for SSO via Authentik.
- **OPDS** — Open Publication Distribution System. Catalog feed standard for ebook/comic readers.
- **OPDS-PSE** — Page Streaming Extension. Allows external readers to stream individual pages over OPDS.
- **PWA** — Progressive Web App. Installable web app with offline support via service worker.
- **RSC** — React Server Components. Next.js App Router rendering model.
- **RTL / LTR / TTB** — Reading directions: right-to-left (manga), left-to-right (Western), top-to-bottom (webtoon).
- **SeaORM** — Async Rust ORM with relationship support, built on SQLx.
- **series.json** — Per-series metadata file at the series folder root. Mylar3 schema.
- **SSO** — Single Sign-On. Provided by Authentik in this homelab.
- **ts_headline / ts_rank_cd / tsquery / tsvector** — Postgres full-text search primitives.
- **utoipa** — Rust crate that derives OpenAPI specs from Axum handler signatures.
- **apalis** — Rust background-job framework, Redis-backed; runs scan, thumbnail, suggestion-engine, and search-dictionary jobs.
- **argon2id** — Modern password hashing function used for local-mode passwords and OPDS app passwords.
- **CSP** — Content Security Policy. HTTP header constraining what scripts/styles/connections a page may load. Concrete policy in §17.4.
- **CSRF** — Cross-Site Request Forgery. Mitigated via double-submit `X-CSRF-Token` (§17.3).
- **JWKS** — JSON Web Key Set. The OIDC issuer publishes signing keys at this endpoint; the server caches and follows rotations.
- **oasdiff** — CI tool that compares two OpenAPI specs and reports breaking changes (§16.1).
- **PKCE** — Proof Key for Code Exchange. RFC 7636 OAuth flow used for all OIDC code exchanges (§17.1).
- **TOTP** — Time-based One-Time Password (RFC 6238). Optional second factor for local users.
- **WCAG 2.2 AA** — Accessibility conformance target for the web app (§7.4, §16.7).
- **WS ticket** — One-shot 30 s token used to authenticate the WebSocket upgrade (§9.6).