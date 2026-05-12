# OPDS Implementation Readiness Audit

**Date:** 2026-05-12
**Scope:** `/opds/*` surface in `crates/server/src/api/opds.rs` and its dependencies
**Verdict:** OPDS 1.x complete — M1 + M2 + M3 closed (HTTP Basic + per-extension MIME + pagination link rels + Range/206 + OpenSearch description + paginated per-series feed + audit-log downloads + `opds` rate-limit bucket + per-entry Dublin Core / author / category / image / related rels + 16 integration tests). M4+ (personal feeds, PSE, OPDS 2.0, progress sync) are separate releases.

## 1. Executive summary

OPDS is shipped as a **minimal Phase-2 catalog** ([crates/server/src/api/opds.rs](../../crates/server/src/api/opds.rs)) covering 6 atom-XML routes. It is **NOT release-ready as advertised**: the settings UI promises HTTP Basic auth with app passwords ([AppPasswordsCard.tsx:148](../../web/components/settings/AppPasswordsCard.tsx#L148)), but the extractor only accepts Bearer/cookie ([extractor.rs:179-192](../../crates/server/src/auth/extractor.rs#L179-L192)) — so Chunky, KyBook, Panels (all listed by name in the UI) will 401. PSE, OPDS 2.0, signed URLs, correct CBZ/CBR MIME, OpenSearch description doc, dedicated rate-limit bucket, audit logging, and integration tests are all absent.

## 2. Endpoint map

| Method | Path | Handler | Auth | Source |
|---|---|---|---|---|
| GET | `/opds/v1` | `root` | Bearer/cookie | [opds.rs:61](../../crates/server/src/api/opds.rs#L61) |
| GET | `/opds/v1/series` | `series_list` | Bearer/cookie | [opds.rs:95](../../crates/server/src/api/opds.rs#L95) |
| GET | `/opds/v1/series/{id}` | `series_one` | Bearer/cookie | [opds.rs:169](../../crates/server/src/api/opds.rs#L169) |
| GET | `/opds/v1/recent` | `recent` | Bearer/cookie | [opds.rs:201](../../crates/server/src/api/opds.rs#L201) |
| GET | `/opds/v1/search?q=` | `search` | Bearer/cookie | [opds.rs:226](../../crates/server/src/api/opds.rs#L226) |
| GET | `/opds/v1/issues/{id}/file` | `download` | Bearer/cookie | [opds.rs:302](../../crates/server/src/api/opds.rs#L302) |

Routes mounted at [app.rs:458](../../crates/server/src/app.rs#L458) (after CSRF; GETs CSRF-exempt by method).

## 3. Feature parity matrix

| Feature | Web | OPDS | Status |
|---|---|---|---|
| Browse series (paginated) | yes | `/opds/v1/series`, page-size 50 | **Complete** |
| Series detail / issue list | yes | `/opds/v1/series/{id}` no pagination | **Partial** |
| Issue metadata (creators / dates / DC) | full | only `title`+`summary`+`updated` | **Partial** |
| Search | tsvector | `LIKE '%q%'` ([opds.rs:252](../../crates/server/src/api/opds.rs#L252)) | **Partial** |
| OpenSearch description doc | n/a | only inline `rel="search"` | **Missing** |
| Recently added | rails | `/opds/v1/recent` | **Complete** |
| Saved views (filter) | yes | none | **Missing** |
| CBL / reading lists | yes | none | **Missing** |
| Pinned rails / home | yes | none | **Missing** |
| Recently read | yes | none | **Missing** |
| Markers / bookmarks | yes | none | **Missing** |
| Want-to-read / favorites | yes | none | **Missing** |
| Read-progress sync (PSE / KEPUB) | yes | none | **Missing** |
| Cover thumbnail link | yes | `…/image/thumbnail` only | **Partial** (no full-size `…/image`) |
| Download acquisition link | yes | correct rel, wrong MIME | **Partial** |
| Range / `Accept-Ranges` | yes (pages) | declares it but ignores `Range` ([opds.rs:327,335](../../crates/server/src/api/opds.rs#L327)) | **Partial** |
| Pagination link rels | n/a | only `next` ([opds.rs:155](../../crates/server/src/api/opds.rs#L155)) | **Partial** |
| Per-user library ACL | enforced | enforced ([opds.rs:103,179,311](../../crates/server/src/api/opds.rs#L103)) | **Complete** |
| Audit log of downloads | n/a | none | **Missing** |
| Rate-limit bucket | per-route | not registered ([rate_limit.rs:82-137](../../crates/server/src/middleware/rate_limit.rs#L82-L137)); advertised in docs | **Doc drift** |
| HTTP Basic auth (in UI) | n/a | not implemented | **Missing — P0** |

## 4. Implementation map by file

- [crates/server/src/api/opds.rs](../../crates/server/src/api/opds.rs) — entire OPDS surface (handlers, XML escape, ACL gates).
- [crates/server/src/api/mod.rs:21](../../crates/server/src/api/mod.rs#L21) — module registration.
- [crates/server/src/app.rs:458](../../crates/server/src/app.rs#L458) — router merge.
- [crates/server/src/auth/extractor.rs:90-92,126-146](../../crates/server/src/auth/extractor.rs#L90-L146) — Bearer `app_…` branch, the only OPDS-relevant auth path that works.
- [crates/server/src/auth/app_password.rs](../../crates/server/src/auth/app_password.rs) — token issue + verify (argon2id + pepper).
- [crates/migration/src/m20260513_000001_app_passwords.rs](../../crates/migration/src/m20260513_000001_app_passwords.rs) — `app_password` table.
- [web/components/settings/AppPasswordsCard.tsx:137-167](../../web/components/settings/AppPasswordsCard.tsx#L137-L167) — sole end-user surface; overstates capabilities.

## 5. OPDS compliance notes

- **Version**: OPDS 1.2 (Atom XML) only. No OPDS 2.0 / JSON-LD.
- **MIME on wire**: generic `application/atom+xml; charset=utf-8`; `link rel="self"` correctly uses `…;kind=navigation` / `…;kind=acquisition` ([opds.rs:36-38,69-71](../../crates/server/src/api/opds.rs#L36-L71)).
- **Present rels**: `self`, `start`, `up`, `subsection`, `search`, `next`, `http://opds-spec.org/sort/new`, `…/acquisition`, `…/image/thumbnail`.
- **Missing rels**: `previous`, `first`, `last`, `http://opds-spec.org/image`, `…/featured`, `…/crawlable`.
- **Acquisition MIME**: `application/zip` for every download ([opds.rs:329,364](../../crates/server/src/api/opds.rs#L329)). Should branch by extension to `application/vnd.comicbook+zip` (CBZ), `application/vnd.comicbook-rar`, `application/x-cbt`, `application/x-cb7`, `application/pdf`, `application/epub+zip`.
- **OpenSearch description doc**: not implemented; only an inline template at [opds.rs:71](../../crates/server/src/api/opds.rs#L71). Chunky / KOReader expect a separate `application/opensearchdescription+xml` document.
- **PSE & signed URLs**: not present. Spec §8.3 of [comic-reader-spec.md](../../comic-reader-spec.md) and [docs/install/caddy.md:37](../install/caddy.md#L37) route `/opds/pse/*` to nothing.
- **Entry metadata gaps**: no `<dc:identifier>` / `<dc:language>` / `<dc:publisher>`, no `<author>`, `<category>`, or `<published>`.
- **XSS safety**: hand-rolled escape covers `& < > " '` ([opds.rs:457](../../crates/server/src/api/opds.rs#L457)) — adequate.

## 6. Client compatibility notes

UI advertises Chunky, KyBook, Panels, Kavita-mobile ([AppPasswordsCard.tsx:148](../../web/components/settings/AppPasswordsCard.tsx#L148)). Expected real behavior today:

- **Chunky / Panels / KyBook 3**: default to HTTP Basic — **fail** (401). Even if reconfigured for Bearer, would reject CBR/CB7 due to MIME mismatch.
- **KOReader**: works for browse via manual Bearer; CBR/CB7 downloads rejected.
- **Foliate / Thorium**: ebook-focused; would parse the feed but find nothing useful (no `application/epub+zip`).
- **Anything PSE-only**: fail.

## 7. Access-control review

- **Auth methods accepted on `/opds/*`**: cookie + Bearer (JWT or `app_…`). **No HTTP Basic** ([extractor.rs:179-192](../../crates/server/src/auth/extractor.rs#L179-L192)).
- **Per-user library visibility**: enforced via `allowed_libraries` + `visible()` against `library_user_access` ([opds.rs:103,179,311,402,416](../../crates/server/src/api/opds.rs#L402-L416)). Admin bypass ([opds.rs:405,417](../../crates/server/src/api/opds.rs#L405-L417)).
- **Download permission**: gated by same `visible()` predicate ([opds.rs:311](../../crates/server/src/api/opds.rs#L311)).
- **CSRF**: GET-only, exempt by method ([app.rs:459](../../crates/server/src/app.rs#L459)). OK.
- **Failed-auth IP lockout** advertised at [docs/architecture/rate-limits.md:36](../architecture/rate-limits.md#L36) does **not** apply to `/opds/*` — no entry in [middleware/rate_limit.rs:82-137](../../crates/server/src/middleware/rate_limit.rs#L82-L137).

## 8. Test coverage matrix

| Surface | Test |
|---|---|
| Root nav | none |
| Series list / pagination | none |
| Series detail | none |
| Recent | none |
| Search | none |
| Download | none |
| ACL gate | none |
| MIME / link rel shape | none |

**Zero OPDS integration tests**: no `*opds*` file in [crates/server/tests/](../../crates/server/tests/). Adjacent [tests/app_passwords.rs](../../crates/server/tests/app_passwords.rs) covers Bearer flow via `/auth/me` only.

## 9. Remaining work (prioritized)

### P0 — blocks "OPDS shipped"

1. **Decode HTTP Basic** in `extract_token` ([extractor.rs:179-192](../../crates/server/src/auth/extractor.rs#L179-L192)). Treat password as `app_…`, ignore username. Without this, every client the UI names will 401.
2. **Correct acquisition MIME** per file extension ([opds.rs:329,364](../../crates/server/src/api/opds.rs#L329)) — CBZ / CBR / CB7 / CBT / PDF / EPUB.
3. **Add OPDS integration tests**: ACL filter, search shape, download MIME, Bearer + Basic round-trip.

### P1 — parity / correctness

4. Paginate `/opds/v1/series/{id}` ([opds.rs:182-190](../../crates/server/src/api/opds.rs#L182-L190)).
5. Emit `prev` / `first` / `last` link rels ([opds.rs:155](../../crates/server/src/api/opds.rs#L155)).
6. Honor `Range` on `/opds/v1/issues/{id}/file` or drop `Accept-Ranges` header ([opds.rs:327-335](../../crates/server/src/api/opds.rs#L327-L335)).
7. Implement an OpenSearch description document at `/opds/v1/search.xml`, link via `rel="search"` ([opds.rs:71](../../crates/server/src/api/opds.rs#L71)).
8. Register an `opds` rate-limit bucket per [rate-limits.md:24](../architecture/rate-limits.md#L24) in [middleware/rate_limit.rs](../../crates/server/src/middleware/rate_limit.rs).
9. Audit-log downloads via `crate::audit::record` for parity with other sensitive surfaces.
10. Enrich entry metadata: authors, publisher, dc:identifier, language, published date, full-size `…/image` rel.

### P2 — Phase-6 catch-up

11. OPDS 2.0 (JSON-LD) at `/opds/v2/*`.
12. PSE (`rel="http://vaemendis.net/opds-pse/stream"`, `pse:count`, `{pageNumber}` template).
13. Signed PSE URLs (spec §8.3: `/opds/pse/{issue_id}/{n}?sig=&exp=&u=`).
14. App-password scopes (`read` vs `read+progress`) and progress-sync feed.
15. Surface saved views / CBL / favorites as navigation subsections.

## 10. Release-readiness verdict

**OPDS complete: NO (partial).** What's wired is a competent Phase-2 catalog with correct ACL plumbing and clean XML escaping ([opds.rs:401-428,457](../../crates/server/src/api/opds.rs#L401-L457)), but it cannot be released against the marketing in [AppPasswordsCard.tsx:148](../../web/components/settings/AppPasswordsCard.tsx#L148): the most popular OPDS clients default to Basic and there is no Basic decoder in the extractor. Combined with hard-coded `application/zip` downloads, no OpenSearch description doc, missing PSE, zero integration tests, and missing rate-limit / lockout middleware that the architecture docs already advertise, the feature is **functional only for users willing to hand-configure Bearer headers against CBZ-only libraries**. Treat as internal alpha until the P0 list is closed.

## 11. Execution checklist

A minimal "ship OPDS 1.x" pass closes the P0 items and at least #5–#7 from P1:

- [x] **P0-1** HTTP Basic decoder in `extract_token` — landed M1; rejects non-`app_…` Basic tokens (footgun guard) ([extractor.rs](../../crates/server/src/auth/extractor.rs))
- [x] **P0-2** MIME-per-extension in `download` + acquisition `type` attribute ([opds.rs](../../crates/server/src/api/opds.rs))
- [x] **P0-3** `crates/server/tests/opds.rs` — 15 integration tests; M2 added pagination link rels, paginated series detail, Range/206 + 416, OpenSearch description, audit-log row
- [x] **P1-4** paginate `/opds/v1/series/{id}` — landed M2; `PAGE_SIZE=50` with first/prev/next/last rels
- [x] **P1-5** emit `previous` / `first` / `last` link rels — landed M2 via `paginate_links` helper
- [x] **P1-6** range support — landed M2; `bytes=N-M` and `bytes=N-` produce 206 + Content-Range, malformed → 416
- [x] **P1-7** OpenSearch description document at `/opds/v1/search.xml` — landed M2; root nav `rel="search"` now points at it
- [x] **P1-8** register `opds` rate-limit bucket — landed M2; 60/min/IP, burst 60 ([middleware/rate_limit.rs](../../crates/server/src/middleware/rate_limit.rs))
- [x] **P1-9** audit-log downloads via `crate::audit::record` — landed M2; action name `opds.download`
- [x] **P1-10** enrich entry metadata — landed M3; per-entry `<dc:identifier>` (`urn:folio:issue:…`), `<dc:language>`, `<dc:publisher>`, `<dc:issued>` (ISO 8601 partial dates), `<author><name>` (first CSV field of `writer`), `<category term=… label=…>` (genre + tags, de-duped), full-size `…/image` rel distinct from thumbnail, plus `rel="related"` deep-link to `/series/{slug}`. Feed root carries `xmlns:dc`.
- [x] Reconcile doc drift in [docs/architecture/threat-model.md](../architecture/threat-model.md) (M1) and [docs/architecture/rate-limits.md](../architecture/rate-limits.md) (M1+M2)
