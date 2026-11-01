# Rate Limits

Phase 0 deliverable. Source: [comic-reader-spec.md §17.7](../../comic-reader-spec.md).

Implementation: `tower_governor` token-bucket ([`middleware/rate_limit.rs`](../../crates/server/src/middleware/rate_limit.rs)), in-memory per process (single replica). Bucket sizes are compile-time constants. The failed-auth lockout and the CBL import quota are Redis-backed and separate.

## Identity resolution

- **Per-user** (`Bucket::build_per_user`, key `UserKey`) = the `sub` of a JWT access token (Bearer, else the session cookie, the same rule `CurrentUser` uses) whose signature, issuer and audience check out. Expiry and the `users` row aren't checked, since the handler's extractor still authenticates: an expired token spends its own user's budget and still gets the 401 that triggers a refresh. A request with no signed JWT (anonymous, forged, app-password, Basic) falls back to the per-IP key, so a forged token can't spend another user's budget.
- **Per-IP** = first untrusted hop in `X-Forwarded-For`, computed by walking the chain right-to-left and stopping at the first IP **not** in `COMIC_TRUSTED_PROXIES`. If all hops are trusted (impossible in practice), the connection's peer address is used.

Without `COMIC_TRUSTED_PROXIES` set, every request appears to come from the reverse proxy — the per-IP bucket fails open. **Required configuration for any prod deploy.**

## Buckets

| Bucket | Per-IP | Per-user | Burst |
|---|---|---|---|
| `GET /issues/.../pages/.*` | 60 req/s | 300 req/s | 120 / 600 |
| `GET /issues/.../pages/.*/thumb` | 200 req/s | 1000 req/s | 400 / 2000 |
| Free-text search, metadata PATCH, collection writes and reorder | — | see [Per-user web-app buckets](#per-user-web-app-buckets) | — |
| `POST /libraries/{id}/scan` | 1 / 5 min | 1 / 5 min | — |
| `POST /auth/oidc/callback`, `POST /auth/local/login` | 5 / min | — | 10 |
| OPDS catalog / navigation / search (`/opds/v1/*`, `/opds/v2/*` except the rows below) — bucket `opds_catalog` | 30 / min | — | 60 |
| OPDS reading path — downloads (`…/issues/{id}/file`, Komga-shape alias), PSE pages (`/opds/pse/*`), progress writes incl. KOReader sync — bucket `opds_stream` (independent of `opds_catalog`, security audit H-3.2) | 120 / min | — | 120 |
| CBL imports — create (upload / URL / catalog), manual refresh, manual check (`cbl::quota`; admins exempt; Redis fixed 1 h window) | — | 20 / hour | — |
| Failed-auth (any) | 10 / min / IP | — | — |
| `POST /csp-report` | 100 / min | — | — |
| `POST /ws/ticket` | 30 / s | — | — |
| `POST /auth/local/request-password-reset` | 5 / hour | 3 / hour / email | 5 / 5 |
| `POST /auth/local/resend-verification` | 5 / hour | 3 / hour / email | 5 / 5 |
| `POST /auth/local/reset-password` | 10 / hour | — | 10 |
| `GET /me/export` | 6 / min | — | 6 |
| `GET /me/markers/export` | 6 / min | — | 6 |
| `GET /me/holdings.csv` (bucket `holdings_export`) | 6 / min | — | 6 |
| `DELETE /me` (bucket `account_delete`; wrong passwords also count as failed auth) | 1 / min | — | 5 |
| Marker writes (`POST /me/markers`, `PATCH`/`DELETE /me/markers/{id}`, `POST /me/markers/bulk-delete`, `POST /me/markers/restore`), one shared bucket | 10 / s | — | 60 |

## Per-user web-app buckets

Audit 2026-11 N-A5 (WP E3). Keyed per user (above), so household members behind one NAT each get their own budget. Every `build_per_user()` call is an independent token store: routes share a budget only inside one sub-router, so each search family below has its own copy of `search`. A 429 is the standard envelope below; web mutations surface it through `useApiMutation`'s error toast, and search queries fail quietly (TanStack Query doesn't retry a 4xx and query errors don't toast).

| Bucket | Routes | Sustained | Burst | Why it is enough |
|---|---|---|---|---|
| `search` | One instance each: `GET /issues/search` + `GET /issues`; `GET /series`; `GET /people`; `GET /me/markers/search`; `GET /characters`; `GET /teams`; `GET /locations`; `GET /arcs` | 10 / s | 60 | The global search sends one debounced query (200 ms in the modal and top bar, 250 ms on `/search`) to each family, so non-stop typing costs a family at most 5/s. The burst absorbs a `/search` session that pages a grid and flips filters. `/series` and `/issues` also back the library grids; infinite scroll fetches one page per sentinel hit. |
| `metadata_write` | `PATCH /series/{slug}`; `PATCH /series/{series_slug}/issues/{issue_slug}` + `PATCH /me/issues/bulk-metadata` (shared) | 1 / s | 30 | Each UI save is one request. The bulk "Edit metadata" dialog sends the whole selection, up to its 500-id cap, in one PATCH, so editing a 500-issue series costs one token. In a writeback library each save becomes an archive rewrite job. |
| `collection_write` | `POST /me/collections`, `PATCH`/`DELETE /me/collections/{id}`, `POST`/`DELETE` entries, `POST …/members/bulk-add`, `POST …/members/bulk-remove` | 5 / s | 60 | The busiest pattern is toggling Want to Read on card after card, a couple per second. Bulk adds and removes from a selection are one request each. |
| `collection_reorder` | `POST /me/collections/{id}/entries/reorder` | 4 / s | 30 | One request per drop with the full order; nobody drops faster than about once a second. Its own bucket, so a drag session never starves adds and removes. |

`POST /auth/refresh` stays unbucketed on purpose. Not covered yet: the generic `/me/saved-views/{id}` PATCH and DELETE (which also accept collection rows), and the admin-only metadata writes outside the three PATCHes above (field-provenance clear/lock, cover pick, external ids).

## Behavior on bucket exhaustion

- 429 with `Retry-After` (seconds).
- `Content-Type: application/json` body: `{ "error": { "code": "rate_limited", "message": "...", "retry_after_seconds": N } }`.
- Failed-auth bucket triggers a 15 min IP-only OPDS lockout (returned as 401 even with valid creds during that window).

Resource cap (not a rate): a non-admin user may own at most 50 server-fetched CBL lists (`source_kind` `url` or `catalog`) — each one is re-fetched by the hourly refresh sweep when it has a schedule. Over the cap, `POST /me/cbl-lists` returns `422 cbl.remote_list_limit`; uploads don't count.

## Metrics

- `folio_rate_limit_denied_total{bucket="…"}` (the CBL import quota reports as `bucket="cbl_import"`)
- `comic_rate_limit_remaining{bucket="…"}` (gauge, sampled per request)

Alert when `folio_rate_limit_denied_total` for any auth bucket exceeds 100/hour — likely brute force in progress.

## Tuning

Defaults are conservative for single-tenant-to-small-family scale (the spec's primary user, §1.3). For larger deployments:

- Raise per-user buckets first; per-IP defenses are independent.
- Search and autocomplete are cheap to relax; page bytes are I/O-bound, raise carefully.
- Auth endpoints should stay tight regardless of scale.

Override via env (planned; not yet implemented in Phase 0):

```
COMIC_RATE_LIMIT_OVERRIDE=pages_read=120/600,pages_thumb=400/2000
```

Form is `bucket_name=per_ip/per_user`. PR-able once the buckets stabilize in real-world use.
