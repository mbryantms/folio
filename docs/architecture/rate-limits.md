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
| Free-text search, metadata writes, collection and saved-view writes, saved-view previews, collection reorder | see [Per-user web-app buckets](#per-user-web-app-buckets) | user | | |
| Page bytes, page thumbnails and stored covers | see [Page bytes and thumbnails](#page-bytes-and-thumbnails) | user | | |
| CBL imports: create (upload / URL / catalog), manual refresh, manual check (`cbl::quota`; admins exempt; Redis fixed 1 h window) | `cbl_import` | user | 20 / hour | none |
| `PATCH /me/account` requests that change the email while mail delivery is configured (see [Email-change requests](#email-change-requests)) | none (request row) | user | 5 / hour | none |

No limit: library and series scans (`POST /libraries/{slug}/scan`, `POST /series/{slug}/scan`), OPDS progression (`/opds/v1/progression/{id}`), the Komga shim (`/api/v1/books/*`), `POST /auth/refresh`, and every other route not in this table.

### Failed-auth lockout

Redis-backed ([`auth/failed_auth.rs`](../../crates/server/src/auth/failed_auth.rs)), separate from the token buckets. Two axes, each 10 failures inside a sliding 60 s window, then a 15 min lockout:

| Applied on | Axes |
|---|---|
| `POST /auth/local/login` | client IP and the submitted email (hashed) |
| `GET /auth/oidc/callback` | client IP |
| `DELETE /me` (password re-check) | client IP and the account's email |
| `PATCH /me/account` email or password change (password re-check, shared with `DELETE /me` through `failed_auth::verify_password_counted`) | client IP and the account's email |
| App-password authentication, Basic or Bearer, on the reader routes (`auth::app_password::verify`) | client IP, on its own counter (`auth_fail_app:{ip}` / `auth_lockout_app:{ip}`) |

The handler checks the lockout before any password hashing, and a request refused by the lockout is not counted. On sign-in every refused attempt counts on both axes: an unknown address, a wrong password, and also a correct password on a disabled or unverified account (sign-in verifies the password before it reports the account state, see [auth-model.md](auth-model.md#sign-in-check-order)). A sign-in post refused by the cross-site check never reaches the handler and is not counted. A locked-out request gets 429 `auth.locked_out` with `Retry-After` (the no-JS sign-in form gets a redirect to `/sign-in` with an `auth.locked` error instead).

App-password misses have their own per-IP counter and lockout (`failed_auth::IpCounter::AppPassword`), with the same 10-in-60 s threshold and 15 min lockout. An app-password lockout refuses only app-password verification from that address; sign-in and cookie sessions are unaffected. A sign-in lockout does not refuse app passwords. Counted as an app-password miss: a token that no row matches, including a token that was revoked, or left outside the legacy window, before its row got a digest. Such a miss is charged before the legacy scan's hashing starts, so a request that ends busy or is dropped has still been counted, and a token created before the digest existed counts one miss on its first successful use. Not counted: a revoked token whose row is found by its digest, a token served from the cache of tokens this process already verified, a 503 busy answer with no legacy scan, and a repeat of a token refused within the last 5 min (`app_password::REFUSAL_TTL`), which is answered 401 from an in-process refusal cache (1024 entries) without a query or hashing.

The lockout check runs after both caches: a cached token needs no hashing and keeps working from a locked-out address, the way an existing session outlives a sign-in lockout, while a token not yet verified gets the 429. A reader app sees 401 `auth.invalid` with `WWW-Authenticate` for a wrong token, as before, and the 429 only while its address is locked out. App-password argon2 verifications also pass a global gate of 2 concurrent permits (`app_password::VERIFY_PERMITS`); a request that waits more than 2 s for one gets 503 `auth.busy` with `Retry-After: 1`.

The lockout is not applied to JWT Bearer authentication (a signature check, no password hashing). The `auth.rate_limit_enabled` setting switches the counters off on every row. When Redis is unavailable the lockout fails open. Behind a reverse proxy, set `COMIC_TRUSTED_PROXIES` so the counted address is the client's and not the proxy's.

Verified against the code at `9492ac1b` on 2026-11-10.

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
| `saved_view_preview` | `POST /me/saved-views/preview` + `POST /me/saved-views/preview-issues` (shared) | 2 / s | 20 | Each preview runs an arbitrary filter for up to the 5 s filter statement timeout, so a loop of them is the cheapest way to keep the database busy. The builder previews on an explicit Preview click (there is no keystroke debounce) and disables the button while a preview is in flight, so a person sends one or two a second at most; the burst covers a quick edit-and-preview session. A 429 shows as the builder's inline preview error. |
| `collection_reorder` | `POST /me/collections/{id}/entries/reorder` | 4 / s | 30 | One request per drop with the full order; nobody drops faster than about once a second. Its own bucket, so a drag session never starves adds and removes. |

Each instance is its own store, so a script that rotates across the four `metadata_write` instances gets four times one instance's budget, and one that alternates `/me/collections/{id}` and `/me/saved-views/{id}` gets `collection_write` twice. That is accepted: a shared store would mean threading one built layer through several modules' `routes()`, and each instance alone still caps its surface.

`POST /auth/refresh` stays unbucketed on purpose. The list and read routes on the same routers (`GET /me/saved-views`, the issue and series GETs, `GET …/metadata-overview`, `GET …/external-ids`) stay unbucketed. Not covered yet: `POST /me/saved-views` (it refuses collection rows), the saved-view pin / sidebar / icon / reorder toggles, the admin-only `/admin/saved-views` writes, the series provider ranges (`POST`/`DELETE /series/{slug}/provider-ranges[/{id}]`, `POST …/provider-ranges/detect`), the series relationships (`POST`/`PATCH`/`DELETE /series/{slug}/relationships[/{id}]`) and the per-user page overrides (`/me/issues/{issue_id}/page-overrides`).

Verified against the code at `ed5d9019` on 2026-11-10 (per-user web-app buckets).

## Page bytes and thumbnails

Audit 2026-11 A9. Both buckets are per user with the IP fallback (`build_per_user`), as above. App-password and anonymous requests key by address, so a household whose reader apps use app passwords behind one NAT shares one budget; the numbers leave room for that. The governor runs before the handler: a conditional request that ends in 304 and a request refused for a missing session spend a token like a page read, and a limited request never opens an archive.

| Bucket | Routes | Sustained | Burst | Why it is enough |
|---|---|---|---|---|
| `pages` | `GET /issues/{id}/pages/{n}` (every `?w=` variant, `HEAD` included) | 250 / s | 2,000 | The reader warms 3 pages ahead and 2 behind with at most 4 in flight (`web/lib/reader/use-prefetch.ts`); holding the next-page key turns about 30 pages a second at about 2 new URLs per turn, about 60/s, about 1,000 requests over a 500-page issue. 250/s is 4x that pace and the burst holds the whole sweep twice. The offline downloader keeps 3 fetches in flight with no pacing (`web/lib/pwa/downloads.ts`); its speed is estimated, not measured, at 75–150 pages a second on a LAN, more for small pages. It waits out a 429 instead of failing (below), so a downloader faster than the bucket is slowed, never stopped. |
| `thumbnails` | `GET /issues/{id}/pages/{n}/thumb` and `GET /issues/{id}/covers/{cover_id}` (one store) | 500 / s | 4,000 | The page editor requests every page's thumbnail at once under a fresh URL per open, 500 for a 500-page issue; the reader strip warms 25 on open. Cover grids are virtualized and lazy and grow 60 cards per list page through the `search` bucket (10/s), so new covers arrive at most about 600 a second by construction, a few hundred in a fast fling; inside the installed app the service worker revalidates every paint, so revisited covers count as well. The downloader's strip-thumbnail phase runs at an estimated 300/s on a LAN and waits out a 429 like its pages. The burst holds eight page-editor opens back to back. |

What the clients do on a 429:

- **Offline downloads** wait for `Retry-After` (at least 1 s, at most 30 s) and fetch the same asset again, with 3 fetches still in flight; after 8 consecutive 429s on one asset (`MAX_RATE_LIMIT_RETRIES`) that issue fails as a retryable error and the queue moves on to the next issue.
- **The reader page** asks the server for the failure's status (a `HEAD` probe, as it does for 404 / 409 / 410); on a 429 it goes back to loading, waits for `Retry-After` and loads again, up to 3 times (`MAX_PAGE_RATE_LIMIT_WAITS`), and keeps its one silent immediate retry for other failures. Past that it shows Tap to retry.
- **Covers, strip thumbnails and the page editor's thumbnails** cannot read an `<img>` status, so every failed load is re-requested after 1 s and then 3 s (`components/RetryingImg.tsx`) before it shows as failed. A truly missing image costs two extra requests.
- **The service worker** does not cache a 429: it stores only `ok` answers, so a limited revalidation leaves the cached copy in place and an uncached image gets the 429, which the retry above handles.

Other routes that serve page bytes or covers keep their own buckets: OPDS PSE pages (`/opds/pse/{issue_id}/{n}`) and whole-archive downloads (`/opds/v1/issues/{id}/file`, `/opds/v2/issues/{id}/file`, `/opds/v1.2/books/{id}/file/{filename}`) are on `opds_stream`; OPDS feeds link covers to `/issues/{id}/pages/0/thumb`, which is the `thumbnails` bucket (keyed by address for app-password clients); OCR reads page bytes behind `ocr` and `ocr.detect`. The Komga shim (`/api/v1/books/*`) serves no page bytes.

Verified against the code at `0cf46f56` on 2026-11-11 (page bytes and thumbnails).

## Behavior on bucket exhaustion

- 429 with `Retry-After` (seconds, at least 1, equal to the body's `retry_after_seconds`). Until A9 the governor's own unfloored header replaced it, so every bucket that refills faster than once a second sent `Retry-After: 0`.
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
- `folio_auth_lockout_total` and `folio_auth_lockout_email_total` (a lockout was set on the IP or email axis), `folio_auth_lockout_app_password_total` (a lockout was set on the app-password counter)

There is no remaining-tokens gauge.

Alert when `folio_rate_limit_denied_total` for any auth bucket exceeds 100/hour — likely brute force in progress.

## Tuning

Defaults are conservative for single-tenant-to-small-family scale (the spec's primary user, §1.3). For larger deployments:

- Raise per-user buckets first; per-IP defenses are independent.
- Search and autocomplete are cheap to relax; the OPDS reading path (`opds_stream`) is I/O-bound, raise carefully.
- Auth endpoints should stay tight regardless of scale.

There is no runtime override: changing a bucket means changing its constant in `middleware/rate_limit.rs`.

## Key-store pruning

governor keeps one entry per key in each bucket's store and never drops one by itself, so a per-IP store would grow with every address a client rotates through. Every `Bucket::build()` / `build_per_user()` registers its store in a process-wide registry (`rate_limit::LimiterRegistry`), and one background task (`rate_limit::spawn_pruner`, started by `app::serve` next to the metrics upkeep and stopped by the same shutdown token) sweeps it every 60 s (`PRUNE_INTERVAL`): each store drops the keys whose bucket has refilled completely (governor's `retain_recent`, which leaves a key exactly as a fresh one would be) and then releases the freed capacity (`shrink_to_fit`). A key that is still spending tokens is kept with its state, so pruning never hands anyone a refilled bucket early. Between sweeps a store holds at most the keys seen in the last minute plus the ones still refilling.

The registry holds its stores weakly: a router that is dropped frees its stores, and the registry forgets the dead handles on its next registration or sweep. The integration tests build many routers per process and never start the sweep. Unit tests in `rate_limit::tests` pin both properties.

Verified against the code at `c7846c06` on 2026-11-11 (pruning, override removal).
