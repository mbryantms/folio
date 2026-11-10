# Auth Model

Phase 0 deliverable. Source of truth: [comic-reader-spec.md §17.1–§17.3, §9.6](../../comic-reader-spec.md).

## Modes

The sign-in mode is one of `oidc`, `local`, `both`, chosen under **Admin → Auth** (the `auth.mode` setting; `COMIC_AUTH_MODE` seeds it on first boot). The login screen reflects the mode.

When no mode has been chosen, `config::resolve_auth_mode` derives it from the OIDC settings (issuer, client id, client secret), and every reader (sign-in page via `/auth/config`, login and register handlers, OIDC routes, admin page) sees that one effective value:

| Chosen mode | OIDC settings | Effective mode |
|---|---|---|
| none | none set | `local` |
| none | all three set | `both` |
| none | some but not all | refuses to boot / save; names the missing values |
| `local` | any | `local` |
| `oidc` or `both` | all three set | as chosen |
| `oidc` or `both` | any missing | refuses to boot / save; names the missing values |

| Mode | Use case |
|---|---|
| `oidc` | Homelab with Authentik/Keycloak/Dex. The most common deployment. |
| `local` | Single-user / small-family installs without an IdP. argon2id + pepper; no second factor (TOTP is not implemented; the unused `users.totp_secret` column is kept for a possible later version). |
| `both` | Mixed — admin via OIDC, opportunistic local accounts (e.g., a guest reader). |

Verified against the code at `ed5d9019` on 2026-11-10.

## Token shapes

### Access token (JWT, 24 h default)

```json
{
  "iss": "https://comics.example.com",
  "sub": "<user_uuid>",
  "aud": "folio",
  "exp": 1735689900,
  "iat": 1735689000,
  "tv":  17,
  "role": "user"
}
```

`tv` (token_version) is the user's `users.token_version` at issuance. Mismatch → 401, force re-auth.

**TTL: 24 hours** (`COMIC_JWT_ACCESS_TTL`, default `24h`). This is longer than typical JWT advice but deliberate: Folio is a content-consumption app, and being kicked back to sign-in mid-issue is friction with no proportional security win. Revocation is still prompt because:

- The auth extractor re-fetches the user row on every request, so admin disable (`token_version` bump) blocks the next request.
- The refresh cookie rotates on every use, so an exfiltrated access cookie is invalidated at the next `/auth/refresh` regardless of its `exp`.
- Worst-case window for a stolen access cookie is the full 24h, but the attacker would also need the refresh cookie or the CSRF cookie to do anything destructive.

Operators wanting tighter defaults can set `COMIC_JWT_ACCESS_TTL=15m`. The web client transparently calls `/auth/refresh` on 401 so the user-visible experience is unchanged — only `/auth/refresh` QPS increases.

Algorithm: `EdDSA` (Ed25519). Server holds the keypair under `/data/secrets/jwt-ed25519.key`. The audience is hardcoded to `folio`; verification also accepts the legacy `comic-reader` audience from before the rename.

### Refresh token (random opaque, 30 d default)

- 32 bytes, base64url. Never inspectable.
- SHA-256 hash stored in `auth_sessions.refresh_token_hash`.
- Rotated on every use: the session row's hash is overwritten in the same transaction.
- `auth_sessions.ip` + `.user_agent` recorded on issue and refresh (M1) — used by `/me/sessions` (M5) and the audit log. Not enforced as a binding (proxy IPs change).
- `COMIC_JWT_REFRESH_TTL` controls the cookie lifetime; validated at boot to be `>= access TTL`.

## Cookie shape (web)

```
__Host-comic_session=<jwt>;     HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age=86400
__Host-comic_csrf=<csrf>;                 Secure; SameSite=Lax; Path=/; Max-Age=86400
__Host-comic_refresh=<rt>;      HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age=2592000
```

All auth cookies are host-only. The refresh path is `/` (not the narrower `/auth/refresh`) because the Next dev proxy rewrites `/api/auth/refresh` → `/auth/refresh` and browsers compare the cookie's `Path` against the original request URL — a narrower path made the cookie omit from the dev refresh call.

The CSRF cookie is intentionally **not** HttpOnly — JS reads it for the `X-CSRF-Token` header (double-submit). The session cookie is HttpOnly.

## Flows

### Local login

```
POST /auth/local/login {email, password}
   ↓
refuse if the client IP or the email is in failed-auth lockout (429)
   ↓
verify argon2id(password + pepper, hash); user must be active
   ↓
INSERT auth_sessions (refresh_token_hash) VALUES (sha256(rt))
   ↓
mint access (access TTL, 24h default) → set __Host-comic_session
mint refresh (30d) → set __Host-comic_refresh
generate csrf       → set __Host-comic_csrf, return in body
```

Verified against the code at `1c51a486` on 2026-11-10.

### OIDC login (PKCE)

```
GET /auth/oidc/start
   ↓
generate code_verifier + state, store in signed __Host-comic_oidc (HttpOnly, 5min)
   ↓
302 → issuer/authorize?response_type=code&code_challenge=...&state=...
                         &redirect_uri=https://.../auth/oidc/callback

[user authenticates at issuer]

GET /auth/oidc/callback?code=...&state=...
   ↓
verify state matches cookie
   ↓
POST issuer/token  (exchange code + verifier)
   ↓
verify ID token: signature via JWKS, aud, iss, exp, nbf, ±60s skew
   ↓
extract sub, email, email_verified
   ↓
if email_verified IS MISSING: treat as false unless COMIC_OIDC_TRUST_UNVERIFIED_EMAIL=true
   ↓
upsert users (external_id = "oidc:<iss>|<sub>")
   ↓
issue cookie session (same as local)
```

A first OIDC login whose email belongs to a local (password) account is refused with 409 `auth.email_in_use` unless `auth.oidc.link_local_by_verified_email` is on. With it on, `auth/oidc.rs::callback` applies one rule:

- The ID token's `email_verified` is not the JSON boolean `true` (absent, even when the trust-unverified opt-in admits the login, or `false`; a string `"true"` fails ID-token parsing): no link. A pending local row gets 403 "verify your email first", any other row 409.
- A disabled local row is refused (403 `auth.disabled`) before anything is written.
- Otherwise the identity is linked and the account becomes **single sign-on only**, whatever the local row's own `email_verified` says (self-registration without mail sets it with no proof): the local password is dropped, `token_version` is bumped, every refresh session and app password is revoked, `email_verified` is set, and a pending row becomes active. The row update is conditional on the external id and `token_version` that were read; the update, the revocations and the audit row commit in one transaction, and a concurrent change answers a retryable 409 `auth.conflict`. The new session is minted only after the commit.

The `auth.oidc.auto_link` audit row records the account's `role`, `activated`, `password_dropped`, `sessions_revoked` and `app_passwords_revoked`, and the takeover is logged at WARN. A row an earlier build linked while keeping its local password loses that password the same way at its next single sign-on (`auth.oidc.local_password_dropped`); no supported path gives an OIDC-keyed account a password otherwise.

Because a linked account can no longer sign in with a password, keep a separate local admin account with a different email as a fallback for when the identity provider is unavailable.

Verified against the code at `59714b1f` on 2026-11-10.

### Refresh

```
POST /auth/refresh  (with __Host-comic_refresh cookie)
   ↓
look up auth_sessions WHERE refresh_token_hash = sha256(rt); refuse (401) if missing, revoked or expired
   ↓
UPDATE that row with the rotated refresh hash, last_used_at, expiry, IP, user agent;
user must be active, else roll back (401); one transaction
   ↓
mint new access; set new cookies
```

If the lookup misses (an already-rotated token replayed, a race between two tabs, an expired or unknown token), the request gets 401 `auth.invalid` and nothing else happens: the server cannot tell which user the old hash belonged to, so no other session is revoked. Refresh-token reuse detection is not implemented. To cut off every device, revoke all sessions (`POST /me/sessions/revoke-all`, which also bumps `token_version`); a password change revokes every session except the current one.

Verified against the code at `1c51a486` on 2026-11-10.

### CSRF (double-submit)

- Server sets `__Host-comic_csrf=<32B base64>` on `/auth/me`.
- Client reads cookie, includes value as `X-CSRF-Token` on POST/PUT/PATCH/DELETE.
- Server compares cookie value to header; mismatch = 403.
- Bearer-authenticated requests (mobile, API, OPDS-Bearer) skip CSRF entirely — there's no ambient credential to forge.

### WebSocket auth (one-shot ticket)

- `POST /auth/ws-ticket` (cookie + CSRF; also mounted at `/api/auth/ws-ticket`) → server generates a random UUID ticket, stores `(ticket → user_id, role)` in Redis with a 5 min TTL, returns `{ ticket, expires_in: 300 }`.
- Client opens `wss://…/ws/scan-events?ticket=<…>`. Server consumes the ticket atomically (Redis `GETDEL`); a missing or used ticket is 401.
- Without a `ticket` parameter the upgrade falls back to the cookie session, and then requires an `Origin` header equal to the configured public origin (`public_url`), else 403 before the upgrade; a ticket-authenticated upgrade has no Origin requirement. Either way the user row is re-read at upgrade: the account must be active and, for `/ws/scan-events` (the only WebSocket endpoint), an admin.

Verified against the code at `1c51a486` on 2026-11-10.

### Email change

`PATCH /me/account { email, current_password }` (local accounts only; `email_editable` on `/auth/me` is false for identity-provider accounts, which get 403 `auth.email_managed_by_issuer`; so does every account while the server runs OIDC-only, since the confirm link refuses in that mode).

- **Re-auth.** An account with a password must send `current_password` (missing: 422 with a `current_password` field error). The check is `failed_auth::verify_password_counted`, shared with `DELETE /me` and the password branch of the same route: a locked-out IP or account gets 429 `auth.locked_out` before any hashing; a wrong password is 403 `auth.invalid` with a `current_password` field error (not 401, which the web client reads as an expired session) and counts on both failed-auth axes. Only `new_password` makes a password change; a `current_password` sent with an unchanged email is ignored.
- **Address in use.** Only after the password checks out, and under the user-row lock, does a taken address get 409 `conflict` (registration reveals the same). The lookup compares `lower(email)`, like the unique index, which stays the backstop.
- **All or nothing.** Every refusal comes before the first write. The writes run in one transaction that first locks the user row. A request whose password was checked is refused like a wrong password (403 `auth.invalid`) when the locked row's `token_version` or password hash differs from the row the password was verified against (a password change or reset, or sign-out-everywhere, committed meanwhile). Under the lock the address-in-use check runs, then the hourly limit is checked (at most 5 requests per user in a fixed one-hour window, counted only while mail is configured; the 6th gets the standard 429 `rate_limited` envelope with `retry_after_seconds` and `Retry-After`, see [rate-limits.md](rate-limits.md#email-change-requests)), then the user row (display name, password hash, `token_version`), the email-change request row, the session revocation and the `user.account.update` audit row are written and committed together. Mail is sent after the commit.
- **Mail configured** (`smtp_host` set, the signal registration and `/auth/config` use): `users.email` does not change. The user's single `email_change_requests` row is updated in place (or inserted) with a new token, the address and the user's `token_version` after this request (so a password change in the same request does not void the link); the previous link stops working. The confirmation link goes to the new address and a notice to the old one, and the account reports `pending_email` while the request is live: unconsumed, unexpired, and made under the user's current `token_version`. Submitting the account's current address cancels a live request; the row stays, expired, so the hourly count holds.
- **Confirm.** `GET /auth/local/confirm-email-change?token=…`. The token is the password-reset envelope with purpose `EmailChange` and the row's `token_id`, valid 1 h. The handler locks the user row, then the request row (the same order as the PATCH). The row must match the user and token hash and be unconsumed and unexpired. If its `token_version` differs from the user's (a password change or reset, sign-out-everywhere or an admin disable since the request), the row is expired and the link answers `invalid`. If another account holds the address the answer is `taken` (also on a unique-index race; any other database error is `invalid`). Otherwise one transaction sets `email`, marks it verified, bumps `token_version`, consumes the row and revokes every refresh session except the opening browser's, which is kept only when it is a live session of the same user. The redirect is `/settings/account?email_change=` `confirmed` (that session kept), `confirmed_signed_out` (no session of the account kept), `invalid` or `taken`.
- **Mail not configured** (the server logs `COMIC_SMTP_HOST not set`): there is no way to confirm, so after the password check the change applies at once with `email_verified = false`, `token_version` is bumped, the other sessions are revoked and any leftover request row is deleted, in the same transaction as the audit row, and a WARN log line records it. Configure SMTP to get confirmed changes.
- **Audit.** `user.account.update` names the address: `pending_email` with `email_change: requested | cancelled`, or `email` + `previous_email` for an immediate change, alongside `password: changed` and `display_name` when those change. `user.account.email_change.confirm` records `email`, `previous_email` and whether the opening browser's session was kept.

Verified against the code at `ffaa3249` on 2026-11-10.

### Logout

- `POST /auth/logout` revokes the current refresh row (`UPDATE … SET revoked_at = now()`) and clears all three cookies; for an OIDC session it also redirects through the issuer's `end_session_endpoint` when one is published. It does not touch other sessions or `token_version`.
- "Log out everywhere" is a separate call, `POST /me/sessions/revoke-all`: it revokes every refresh row for the user and bumps `users.token_version`.

## Library ACL and the age-rating cap

Non-admin users see a library only through a `library_user_access` row
(admins are unrestricted). Each grant may carry an **age-rating cap**
(`age_rating_max`), set per library from `/admin/users/{id}` → *Library
access*. The cap is enforced server-side on every read surface; the web
app never filters client-side.

### Ladder

The cap is a rung of the ComicInfo `AgeRating` vocabulary, ordered
youngest-audience-first (`crates/server/src/library/age_rating.rs`):

```
Early Childhood < Everyone < G < Everyone 10+ < PG < Kids to Adults < Teen
  < MA15+ < Mature 17+ < M < R18+ < Adults Only 18+ < X18+
```

Comparison is case-insensitive and ignores surrounding whitespace. The
admin API validates a cap against the ladder (422 otherwise) and stores
the canonical spelling.

### What a capped user sees

- A **series** is visible when `series.age_rating` ranks at or below the
  cap.
- An **issue** is visible when `COALESCE(issue.age_rating,
  series.age_rating)` ranks at or below the cap — an issue may carry its
  own rating above its series' and is then hidden on its own.
- **Unrated content is shown** (decision D6, 2026-09-29): `NULL`, empty,
  `Unknown`, `Rating Pending` and any string not on the ladder all pass.
  Only rows whose rating is a known rung *above* the cap are hidden, so
  the SQL predicate is `rating IS NULL OR lower(btrim(rating)) NOT IN
  (<rungs above the cap>)`, never an allow-list.

Denied lookups return the same `404 not_found` an ACL miss returns —
nothing about the hidden row leaks (not its existence, cover, or page
bytes).

### Surfaces enforced

The predicate is carried on `library::access::VisibleLibraries` (built
once per request by `access::for_user`) and applied in three shapes —
`series_filter()` / `issue_filter()` for sea-orm queries,
`raw_cap_clause()` for hand-written SQL, and `series_ok()` /
`issue_ok()` / `access::issue_visible()` for single rows. Every surface
that consults the library ACL applies it:

| Surface | Where |
|---|---|
| Series list + search, series detail, per-series issue list, resume | `api/series.rs` |
| Cross-library issue list, issue search, issue detail + sub-resources | `api/issues.rs` |
| Page bytes, page/cover thumbnails, variant covers, permalinks | `api/page_bytes.rs`, `api/thumbnails.rs`, `api/covers.rs`, `api/issue_permalink.rs` |
| Saved views / smart-filter compiler (home rails, `/views`) | `views/compile.rs` |
| Continue reading, On Deck, New issues, rail dismissals | `api/rails.rs` |
| Reader Up Next / Prev Up (series + CBL walks) | `api/next_up.rs` |
| Creators, people search, filter-option pickers (UNION arms) | `api/creators.rs`, `api/people.rs`, `api/filter_options.rs` |
| Character / team / location / arc / publisher pages, browse indexes, insights | `api/entity_pages.rs` (shared by the OPDS entity feeds) |
| Similar-series reasons, series "Appears in" arcs, reading-stats top creators | `api/series_similar.rs`, `api/appearances.rs`, `api/reading_sessions.rs` |
| CBL entries, markers, ratings, progress writes, reading sessions, OCR | `api/cbl_lists.rs`, `api/markers.rs`, `api/ratings.rs`, `api/progress.rs`, `api/reading_sessions.rs`, `api/issue_ocr.rs` |
| OPDS 1.2 + 2.0 feeds, facets, downloads, PSE page streaming, progression + KOReader sync | `api/opds.rs`, `api/opds_v2.rs`, `api/opds_pse.rs`, `api/opds_progression.rs` |
| Komga-compatible progress shim | `api/komga_compat.rs` |
| Metadata search / external ids / provider ranges (series gates) | `api/metadata_search.rs`, `api/external_ids.rs`, `api/provider_ranges.rs` |

### Entity surfaces

Creators, characters, teams, locations, story arcs, publishers, genres
and tags are shown to a capped caller only through the issues and series
that caller can see. An issue counts when it is not removed, is active,
sits in a non-removed series of a granted library and passes the cap
(`COALESCE(issue.age_rating, series.age_rating)`, unrated passes), and
its series' own rating passes the cap too
(`VisibleLibraries::entity_issue_cap_sql`): entity surfaces never name a
series the caller cannot open, even through an issue rated below it.

- **Issue junctions** (`issue_credits`, `issue_characters`,
  `issue_teams`, `issue_locations`, `issue_arcs`, `issue_genres`,
  `issue_tags`): a row counts when its issue counts.
- **Series rollups** (`series_credits`, `series_characters`,
  `series_teams`, `series_locations`, `series_genres`, `series_tags`):
  the scanner rebuilds these as the union of every issue in the series,
  hidden ones included. For a caller whose cap hides any rung
  (`VisibleLibraries::hides_any`) they are not read; the issue junction
  over the issues that caller can see answers instead.
- **Series-level metadata** (`series.publisher`, `series_arcs`,
  `series_universes`, `series_concepts`, `series_objects`,
  `series_group`): describes the series itself and follows series
  visibility.

So an entity is listed, searchable, offered as a filter value or counted
only when something visible links to it; its detail routes
(`/<kind>/{slug}` and the `/series`, `/issues`, `/insights` sub-routes,
`/creators/{slug}`) answer the same 404 as an unknown slug otherwise.
Counts, roles, year ranges, co-appearances and top-N lists cover visible
issues only, and a creator's role held only on hidden issues is not
listed. The same rule covers the browse indexes and their `?q=` search,
`/people`, the filter-option pickers, the series-list credit, genre and
tag filters, the series saved-view filters on credits, characters, teams,
locations, genres and tags, the series "Appears in" arcs, OPDS author and genre facets
and the by-creator feeds, the reading-stats top creators and the
similar-series reasons (see
[`docs/dev/similar-series.md`](../dev/similar-series.md#acl-and-per-user-filtering)).
Admins and callers without a cap that hides anything keep the rollup
queries unchanged.

Regression suite: `crates/server/tests/age_rating_cap.rs` (a `Teen`-capped
user against `Teen` / `Mature 17+` / unrated series, plus an issue-level
override) and the unit tests in `library/age_rating.rs` / `library/access.rs`.

### The removed `role` column

`library_user_access.role` (`reader | curator`) existed from Phase 1 but
was never read; WP-2.7 dropped it (`m20270123_000001_drop_library_access_role`)
and removed the field from the admin users API. A per-library editor role
can be re-introduced with real semantics later.

## Failure modes

| Symptom | Likely cause |
|---|---|
| Loop: cookie set, immediately rejected | `COMIC_PUBLIC_URL` doesn't match the host the cookie was set on (`__Host-` requires exact host). |
| OIDC callback 401 with valid token | Clock skew > 60 s; check NTP on the app host. |
| One tab or device signed out unexpectedly | Its refresh token was already rotated by another request (two tabs refreshing at once) or the session expired or was revoked; `/auth/refresh` answers 401 for that token only. |
| All sessions invalidated | A password reset, an admin disable, or "revoke all sessions" (each bumps `token_version` and revokes sessions; a self-serve password change does the same but keeps the current session). Refresh-token replay never does this. |
| OIDC user marked unverified | `email_verified` claim missing from the issuer; fix the Authentik mapping (`docs/install/authentik.md`) or set the trust flag. |
