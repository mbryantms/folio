# Rate Limits

Phase 0 deliverable. Source: [comic-reader-spec.md §17.7](../../comic-reader-spec.md).

Implementation: `tower_governor` token-bucket ([`middleware/rate_limit.rs`](../../crates/server/src/middleware/rate_limit.rs)), in-memory per process (single replica). Bucket sizes are compile-time constants. The failed-auth lockout and the CBL import quota are Redis-backed and separate.

## Identity resolution

- **Per-user** (`Bucket::build_per_user`, key `UserKey`) = the `sub` of a JWT access token (Bearer, else the session cookie, the same rule `CurrentUser` uses) whose signature, issuer and audience check out. Expiry and the `users` row aren't checked, since the handler's extractor still authenticates: an expired token spends its own user's budget and still gets the 401 that triggers a refresh. A request with no signed JWT (anonymous, forged, app-password, Basic) falls back to the per-IP key, so a forged token can't spend another user's budget.
- **Per-IP** = first untrusted hop in `X-Forwarded-For`, computed by walking the chain right-to-left and stopping at the first IP **not** in `COMIC_TRUSTED_PROXIES`. If all hops are trusted (impossible in practice), the connection's peer address is used.

Without `COMIC_TRUSTED_PROXIES` set, every request appears to come from the reverse proxy — the per-IP bucket fails open. **Required configuration for any prod deploy.**

## Buckets

Every limit that is attached to a route today. A route not listed here has no limit. JSON routes are served under `/api` (the table drops the prefix); `auth::local` and the WebSocket ticket are mounted both bare and under `/api`, and each mount has its own bucket.

| Route(s) | Bucket | Key | Sustained | Burst |
|---|---|---|---|---|
| `POST /auth/local/login` | `auth.login` | IP | 5 / min | 10 |
| `POST /auth/local/register` | `auth.register` | IP | 5 / min | 10 |
| `GET /auth/oidc/callback` | `auth.oidc_callback` | IP | 5 / min | 10 |
| `POST /admin/auth/oidc/discover` (separate instance of the same constants) | `auth.oidc_callback` | IP | 5 / min | 10 |
| `POST /auth/local/request-password-reset` | `auth.password_reset_request` | IP | 5 / hour | 5 |
| `POST /auth/local/resend-verification` + `GET /auth/local/verify-email` (shared) | `auth.resend_verification` | IP | 5 / hour | 5 |
| `POST /auth/local/reset-password` | `auth.password_reset_redeem` | IP | 10 / hour | 10 |
| `GET /auth/local/confirm-email-change` (own store) | `auth.password_reset_redeem` | IP | 10 / hour | 10 |
| `POST /auth/ws-ticket` | `ws_ticket` | IP | 30 / s | 60 |
| `POST /csp-report` | `csp_report` | IP | 100 / min | 200 |
| OPDS catalog / navigation / search (`/opds/v1/*`, `/opds/v2/*` except the row below) | `opds_catalog` | IP | 30 / min | 60 |
| OPDS reading path: downloads (`…/issues/{id}/file`, `/opds/v1.2/books/{id}/file/{filename}`), PSE pages (`/opds/pse/*`), progress writes (`PUT …/issues/{id}/progress`, KOReader `PUT /opds/v1/syncs/progress/{hash}`); independent of `opds_catalog` (security audit H-3.2) | `opds_stream` | IP | 120 / min | 120 |
| `POST /me/issues/{id}/ocr` | `ocr` | IP | 60 / min | 60 |
| `GET /me/issues/{id}/pages/{page}/text-regions` | `ocr.detect` | IP | 15 / min | 10 |
| `GET /me/export` | `user_export` | IP | 6 / min | 6 |
| `GET /me/markers/export` | `notes_export` | IP | 6 / min | 6 |
| `GET /me/holdings.csv` | `holdings_export` | IP | 6 / min | 6 |
| `DELETE /me` (wrong passwords also count as failed auth) | `account_delete` | IP | 1 / min | 5 |
| Marker writes (`POST /me/markers`, `PATCH`/`DELETE /me/markers/{id}`, `POST /me/markers/bulk-delete`, `POST /me/markers/restore`), one shared bucket | `marker_write` | IP | 10 / s | 60 |
| Free-text search, metadata writes, collection and saved-view writes, collection reorder | see [Per-user web-app buckets](#per-user-web-app-buckets) | user | | |
| CBL imports: create (upload / URL / catalog), manual refresh, manual check (`cbl::quota`; admins exempt; Redis fixed 1 h window) | `cbl_import` | user | 20 / hour | none |
| `PATCH /me/account` requests that change the email while mail delivery is configured (see [Email-change requests](#email-change-requests)) | none (request row) | user | 5 / hour | none |

No limit: page bytes (`GET /issues/{id}/pages/{n}`), thumbnails and stored covers (`…/pages/{n}/thumb`, `/issues/{id}/covers/{cover_id}`), library and series scans (`POST /libraries/{slug}/scan`, `POST /series/{slug}/scan`), OPDS progression (`/opds/v1/progression/{id}`), the Komga shim (`/api/v1/books/*`), `POST /auth/refresh`, and every other route not in this table.

### Failed-auth lockout

Redis-backed ([`auth/failed_auth.rs`](../../crates/server/src/auth/failed_auth.rs)), separate from the token buckets. Two axes, each 10 failures inside a sliding 60 s window, then a 15 min lockout:

| Applied on | Axes |
|---|---|
| `POST /auth/local/login` | client IP and the submitted email (hashed) |
| `GET /auth/oidc/callback` | client IP |
| `DELETE /me` (password re-check) | client IP and the account's email |
| `PATCH /me/account` email or password change (password re-check, shared with `DELETE /me` through `failed_auth::verify_password_counted`) | client IP and the account's email |

The handler checks the lockout before any password hashing. On sign-in every refused attempt counts on both axes: an unknown address, a wrong password, and also a correct password on a disabled or unverified account (sign-in verifies the password before it reports the account state, see [auth-model.md](auth-model.md#sign-in-check-order)). A locked-out request gets 429 `auth.locked_out` with `Retry-After` (the no-JS sign-in form gets a redirect to `/sign-in` with an `auth.locked` error instead). The lockout is not applied to OPDS, app-password or Bearer authentication (not implemented). When Redis is unavailable the lockout fails open.

Verified against the code at `5a1d2f64` on 2026-11-10.

### Email-change requests

Each accepted email change sends two emails (the link to the new address, a notice to the old one), so a user may make at most 5 requests per hour while mail delivery is configured ([`api/account.rs`](../../crates/server/src/api/account.rs), `EMAIL_CHANGE_REQUESTS_PER_HOUR`). Without mail delivery an email change applies at once and sends nothing, so it is not counted. Not a token bucket: the route also carries display-name and password edits, which this limit must not touch.

The count lives on the user's single `email_change_requests` row (`window_started_at`, `request_count`). The window is fixed: it starts at the first request and lasts one hour. A new request updates that row in place (new token, new address, count + 1); a cancel expires the row but keeps it, and a credential change only makes it non-live (its `token_version` no longer matches the account's; the row is marked expired when its link is next opened), so the count is never lost (only an immediate change without mail delivery deletes the row, and nothing is counted then). The handler checks the limit inside the PATCH's transaction after locking the user row (`SELECT … FOR UPDATE`), which serializes concurrent requests of one account, and before writing anything: a refused request changes nothing, including a password change sent in the same request. The 6th request inside the window gets the standard 429 `rate_limited` envelope below, with `retry_after_seconds` and `Retry-After` set to the rest of the window. Because the count is in Postgres it neither fails open with Redis nor follows the `auth.rate_limit_enabled` switch.

Verified against the code at `ffaa3249` on 2026-11-10.

## Per-user web-app buckets

Audit 2026-11 N-A5 (WP E3, gaps closed in E12). Keyed per user (above), so household members behind one NAT each get their own budget. Every `build_per_user()` call is an independent token store: routes share a budget only inside one sub-router, so each search family below has its own copy of `search`. A 429 is the standard envelope below; web mutations surface it through `useApiMutation`'s error toast, and search queries fail quietly (TanStack Query doesn't retry a 4xx and query errors don't toast).

| Bucket | Routes | Sustained | Burst | Why it is enough |
|---|---|---|---|---|
| `search` | One instance each: `GET /issues/search` + `GET /issues`; `GET /series`; `GET /people`; `GET /me/markers/search`; `GET /characters`; `GET /teams`; `GET /locations`; `GET /arcs` | 10 / s | 60 | The global search sends one debounced query (200 ms in the modal and top bar, 250 ms on `/search`) to each family, so non-stop typing costs a family at most 5/s. The burst absorbs a `/search` session that pages a grid and flips filters. `/series` and `/issues` also back the library grids; infinite scroll fetches one page per sentinel hit. |
| `metadata_write` | One instance each: `PATCH /series/{slug}`; `PATCH /series/{series_slug}/issues/{issue_slug}` + `PATCH /me/issues/bulk-metadata` + `PUT`/`DELETE /series/{series_slug}/issues/{issue_slug}/field-provenance/{field}` (shared); the cover picks `PATCH /series/{series_slug}/issues/{issue_slug}/cover`, `POST …/cover/variant/{variant_id}`, `DELETE …/cover/variant`, `PATCH /series/{slug}/cover` (shared); the external-id writes `POST`/`DELETE /series/{slug}/external-ids[/{source}]` and `POST`/`DELETE /series/{slug}/issues/{issue_slug}/external-ids[/{source}]` (shared) | 1 / s | 30 | Each UI save is one request. The bulk "Edit metadata" dialog sends the whole selection, up to its 500-id cap, in one PATCH, so editing a 500-issue series costs one token. A field lock, a pin release, a cover pick and an id edit are one click each, and the control is disabled while its request is in flight; no UI loops over fields or ids. In a writeback library each save becomes an archive rewrite job. |
| `collection_write` | `POST /me/collections`, `PATCH`/`DELETE /me/collections/{id}`, `POST`/`DELETE` entries, `POST …/members/bulk-add`, `POST …/members/bulk-remove` | 5 / s | 60 | The busiest pattern is toggling Want to Read on card after card, a couple per second. Bulk adds and removes from a selection are one request each. |
| `saved_view_write` | `PATCH`/`DELETE /me/saved-views/{id}` | 5 / s | 60 | These also edit and delete collection rows, so they get `collection_write`'s numbers. A sibling bucket, not the collections instance: every `build_per_user()` is its own store and the two modules build their routers separately, and the own name tells the denial counter which surface tripped. The web sends one PATCH per explicit save (filter sheet, CBL detail) and one DELETE behind a confirm. |
| `collection_reorder` | `POST /me/collections/{id}/entries/reorder` | 4 / s | 30 | One request per drop with the full order; nobody drops faster than about once a second. Its own bucket, so a drag session never starves adds and removes. |

Each instance is its own store, so a script that rotates across the four `metadata_write` instances gets four times one instance's budget, and one that alternates `/me/collections/{id}` and `/me/saved-views/{id}` gets `collection_write` twice. That is accepted: a shared store would mean threading one built layer through several modules' `routes()`, and each instance alone still caps its surface.

`POST /auth/refresh` stays unbucketed on purpose. The list and read routes on the same routers (`GET /me/saved-views`, the issue and series GETs, `GET …/metadata-overview`, `GET …/external-ids`) stay unbucketed. Not covered yet: `POST /me/saved-views` (it refuses collection rows), the saved-view pin / sidebar / icon / reorder toggles, the admin-only `/admin/saved-views` writes, the series provider ranges (`POST`/`DELETE /series/{slug}/provider-ranges[/{id}]`, `POST …/provider-ranges/detect`), the series relationships (`POST`/`PATCH`/`DELETE /series/{slug}/relationships[/{id}]`) and the per-user page overrides (`/me/issues/{issue_id}/page-overrides`).

## Behavior on bucket exhaustion

- 429 with `Retry-After` (seconds, at least 1).
- `Content-Type: application/json` body: `{ "error": { "code": "rate_limited", "message": "...", "retry_after_seconds": N } }`.
- The CBL import quota and the email-change request limit answer the same 429 shape. The failed-auth lockout answers 429 `auth.locked_out` (see [Failed-auth lockout](#failed-auth-lockout)).
- If a request carries no resolvable client IP (a wiring fault), a per-IP bucket answers 500 `internal`.

## The `auth.rate_limit_enabled` switch

`auth.rate_limit_enabled` (default `true`) is a database setting, editable at `/admin/server` → Hardening and applied live. The deprecated `COMIC_RATE_LIMIT_ENABLED` env var only seeds it on first boot. Setting it to `false` turns off:

- the failed-auth lockout, both recording and checking, on both axes;
- the CBL import quota's hourly window (the 50-list standing cap below stays).

It does not turn off any token bucket in the [Buckets](#buckets) table: those are installed at boot and stay on regardless.

Verified against the code at `1c51a486` on 2026-11-10.

Resource cap (not a rate): a non-admin user may own at most 50 server-fetched CBL lists (`source_kind` `url` or `catalog`) — each one is re-fetched by the hourly refresh sweep when it has a schedule. Over the cap, `POST /me/cbl-lists` returns `422 cbl.remote_list_limit`; uploads don't count.

## Metrics

- `folio_rate_limit_denied_total{bucket="…"}` (the CBL import quota reports as `bucket="cbl_import"`)
- `folio_auth_lockout_total` and `folio_auth_lockout_email_total` (a lockout was set on the IP or email axis)

There is no remaining-tokens gauge.

Alert when `folio_rate_limit_denied_total` for any auth bucket exceeds 100/hour — likely brute force in progress.

## Tuning

Defaults are conservative for single-tenant-to-small-family scale (the spec's primary user, §1.3). For larger deployments:

- Raise per-user buckets first; per-IP defenses are independent.
- Search and autocomplete are cheap to relax; the OPDS reading path (`opds_stream`) is I/O-bound, raise carefully.
- Auth endpoints should stay tight regardless of scale.

Override via env (planned; not yet implemented in Phase 0):

```
COMIC_RATE_LIMIT_OVERRIDE=pages_read=120/600,pages_thumb=400/2000
```

Form is `bucket_name=per_ip/per_user`. PR-able once the buckets stabilize in real-world use.
