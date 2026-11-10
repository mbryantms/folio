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

### Access token (JWT, 1 h default)

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

**TTL: 1 hour** (`auth.jwt.access_ttl` under **Admin → Auth → Tokens**; `COMIC_JWT_ACCESS_TTL` seeds it on first boot; built-in default `1h`, `24h` before this version, and an install that saved a value keeps it). The access token carries no session id and is not looked up per request, so it cannot be recalled: signing out revokes the refresh session, but an access token already issued keeps working until its `exp`. The short TTL bounds that window. Other revocation is prompt:

- The auth extractor re-fetches the user row on every request, so admin disable (state change plus `token_version` bump) blocks the next request.
- A password change or reset and "sign out everywhere" bump `token_version`, which voids every access token at once.

Expiry is normally not visible to the web user. Signed-in pages renew the access token before it lapses ([proactive refresh](#refresh)); a request that still meets an expired cookie gets a 401, and `apiFetch` then calls `POST /auth/refresh` once and retries the request once; a full page load that lands on `/sign-in` because the access cookie expired renews the session there and returns to the page. What can still sign a user out is listed under [Refresh](#refresh).

Verified against the code at `5a1d2f64` on 2026-11-10.

Algorithm: `EdDSA` (Ed25519). Server holds the keypair under `/data/secrets/jwt-ed25519.key`. The audience is hardcoded to `folio`; verification also accepts the legacy `comic-reader` audience from before the rename.

### Refresh token (random opaque, 30 d sliding, 90 d maximum)

- 32 bytes, base64url. Never inspectable.
- SHA-256 hash stored in `auth_sessions.refresh_token_hash`.
- Rotated on every use by one conditional `UPDATE` (see [Refresh](#refresh)); single-use.
- `auth_sessions.ip` + `.user_agent` recorded on issue and refresh (M1) — used by `/me/sessions` (M5) and the audit log. Not enforced as a binding (proxy IPs change).
- Sliding expiry: `auth.jwt.refresh_ttl` (default `30d`, `COMIC_JWT_REFRESH_TTL` seeds it) sets the cookie lifetime and moves `expires_at` forward on every refresh; validated to be `>= access TTL`.
- Absolute age: `auth.session.max_age` (default `90d`, floor `1d`, `>=` access TTL; **Admin → Auth → Tokens**). A session cannot be refreshed past `created_at + max age` however often it is used; `expires_at`, and the Max-Age of the refresh and CSRF cookies, are clamped to that bound at sign-in and on every refresh. `/me/sessions` does not list a session past it. Change it in the admin UI. `COMIC_SESSION_MAX_AGE` is also read, through the generic `COMIC_` env prefix, as the baseline while no row is saved; a saved row wins (see [runtime-configuration.md](../dev/runtime-configuration.md)).

Verified against the code at `5a1d2f64` on 2026-11-10.

## Cookie shape (web)

```
__Host-comic_session=<jwt>;     HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age=3600
__Host-comic_access_exp=<exp>;            Secure; SameSite=Lax; Path=/; Max-Age=3600
__Host-comic_csrf=<csrf>;                 Secure; SameSite=Lax; Path=/; Max-Age=2592000
__Host-comic_refresh=<rt>;      HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age=2592000
```

Max-Age values shown for the defaults. The session and access-expiry cookies live for the access TTL; the refresh and CSRF cookies for the refresh TTL, clamped to what is left of the session's absolute age.

`__Host-comic_access_exp` holds the access token's `exp` (epoch seconds) and is **not** HttpOnly, so the web client can see when the token lapses without reading the token. It holds no secret. It is set wherever the session cookie is set (local login and registration, refresh, the OIDC callback) and cleared with it (logout, account deletion).

All auth cookies are host-only. The refresh path is `/` (not the narrower `/auth/refresh`) because the Next dev proxy rewrites `/api/auth/refresh` → `/auth/refresh` and browsers compare the cookie's `Path` against the original request URL — a narrower path made the cookie omit from the dev refresh call.

The CSRF cookie is intentionally **not** HttpOnly — JS reads it for the `X-CSRF-Token` header (double-submit). The session cookie is HttpOnly. The CSRF cookie lives as long as the refresh cookie (refresh TTL), not the access cookie: `POST /auth/refresh` is CSRF-checked, so it must still be there after the access cookie expired. It is not a credential on its own, and sign-out clears it.

Verified against the code at `5a1d2f64` on 2026-11-10.

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
mint access (access TTL, 1h default) → set __Host-comic_session
mint refresh (30d) → set __Host-comic_refresh
set __Host-comic_access_exp (access expiry; access TTL)
generate csrf       → set __Host-comic_csrf (refresh lifetime), return in body
```

Verified against the code at `5a1d2f64` on 2026-11-10.

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

### Refresh

```
POST /auth/refresh  (with __Host-comic_refresh cookie; CSRF-checked)
   ↓
one transaction:
  UPDATE auth_sessions
     SET refresh_token_hash = sha256(new rt), last_used_at = now(),
         expires_at = LEAST(now() + refresh TTL, created_at + max age),
         ip, user_agent (kept when the request has none)
   WHERE refresh_token_hash = sha256(rt) AND revoked_at IS NULL
     AND expires_at > now() AND created_at + max age > now()
  RETURNING id, user_id, created_at
  exactly one row, else 401 auth.invalid
  user must be active, else roll back (401)
   ↓
mint new access; set new cookies
  (refresh + CSRF Max-Age = min(refresh TTL, created_at + max age - now))
```

The `WHERE` is evaluated under the row lock, so the rotation is atomic: of two concurrent refreshes carrying the same token, one updates the row and the other then finds the rotated hash, matches nothing and gets 401 `auth.invalid`. If nothing matches (an already-rotated token replayed, the loser of such a race, an expired, revoked or unknown token) the request gets 401 `auth.invalid` and only one more thing can happen: when the row exists, is not revoked and is past its maximum age, it is marked revoked (so `/me/sessions` stops listing it). The server cannot tell which user an unknown hash belonged to, so no other session is revoked. Refresh-token reuse detection is not implemented.

The rotation commits before the response leaves the server. If that response is lost (the connection drops, the tab is closed mid-request), the browser keeps the old refresh cookie, which is now dead, and the next refresh is refused: that session is over and the user signs in again. A short grace window accepting the previous token is a possible later change; it is not implemented. To cut off every device, revoke all sessions (`POST /me/sessions/revoke-all`, which also bumps `token_version`); a password change revokes every session except the current one.

**Proactive refresh.** Signed-in pages mount a scheduler ([`web/lib/api/session-refresh.ts`](../../web/lib/api/session-refresh.ts), mounted once by the root layout for a signed-in request, inert on the auth pages). It reads `__Host-comic_access_exp` and refreshes at about 80 % of the remaining lifetime, never later than 30 s before expiry (with the 1 h default: about 48 min after each renewal). It re-arms after every refresh, its own or another tab's, when the page becomes visible again and when the browser comes back online; a page that becomes visible with the token near or past expiry refreshes at once. Without the CSRF cookie (signed out) it arms nothing. Requests the 401 retry cannot cover call `ensureFreshSession()` first, which resolves at once when the token is fresh and otherwise waits for a refresh (promptly when offline or on a network failure): the reader's page-image retry and Tap to retry, a prefetch batch near expiry, and one retry of an offline-download asset that got 401.

**Two tabs.** Tabs share cookies, so the tab that loses a refresh race already holds the winner's new tokens. The web client ([`web/lib/api/auth-refresh.ts`](../../web/lib/api/auth-refresh.ts)) serializes refreshes across tabs with the Web Locks API (`folio:auth-refresh`). Whether another tab refreshed is read from the access-expiry cookie: it counts only when the cookie is present, later than the value this tab saw before it tried, and in the future. A missing cookie never counts (sign-out clears it), and a changed CSRF cookie alone means nothing (the CSRF cookie is also re-minted by `/auth/me` and by password changes). Inside the lock, a tab that sees the moved expiry skips its own refresh. Without Web Locks, a 401 or 403 from `/auth/refresh` counts as a lost race when the expiry moved, checked again after 300 ms; either way the tab retries its original request once with the new cookies. Only when the expiry did not move is the session treated as gone, and the client then expires the CSRF and access-expiry cookies itself so nothing keeps retrying a dead session. A refresh that times out (15 s), fails on the network or gets a 5xx is "no answer": nothing is signed out and no private state is cleared.

**Full page loads.** Server-rendered pages cannot refresh. A page load whose access cookie expired is redirected to `/sign-in?next=…`; when a CSRF cookie is present (a session may exist) and the URL does not carry `signed_out=1`, the sign-in page first shows "Signing you in…" (rendered on the server, so the form does not flash), calls the same refresh and, on success, returns to `next`. Loop guard: every attempt, successful or not, is stamped in sessionStorage and the page does not try again within 30 s; without sessionStorage it tries at most once per page load. A refused refresh also expires the CSRF cookie, so the next visit does not try at all.

**What can still sign a user out:** sign-out or "sign out everywhere"; a session past its maximum age, expired (unused for the refresh TTL) or revoked; an account disabled; a password change on another session; a lost refresh response (above); two separate clients sharing one refresh token; and a browser that stays asleep or offline past the refresh lifetime.

Verified against the code at `5a1d2f64` on 2026-11-10.

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

- `POST /auth/logout` revokes the current refresh row (`UPDATE … SET revoked_at = now()`) and clears the auth cookies (session, access expiry, CSRF, refresh); for an OIDC session it also redirects through the issuer's `end_session_endpoint` when one is published. It does not touch other sessions or `token_version`, so a copy of the access token taken before sign-out stays valid until it expires (at most the access TTL, 1 h by default).
- "Log out everywhere" is a separate call, `POST /me/sessions/revoke-all`: it revokes every refresh row for the user and bumps `users.token_version`.

Verified against the code at `5a1d2f64` on 2026-11-10.

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
| CBL entries, markers, ratings, progress writes, reading sessions, OCR | `api/cbl_lists.rs`, `api/markers.rs`, `api/ratings.rs`, `api/progress.rs`, `api/reading_sessions.rs`, `api/issue_ocr.rs` |
| OPDS 1.2 + 2.0 feeds, facets, downloads, PSE page streaming, progression + KOReader sync | `api/opds.rs`, `api/opds_v2.rs`, `api/opds_pse.rs`, `api/opds_progression.rs` |
| Komga-compatible progress shim | `api/komga_compat.rs` |
| Metadata search / external ids / provider ranges (series gates) | `api/metadata_search.rs`, `api/external_ids.rs`, `api/provider_ranges.rs` |

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
| One tab or device signed out unexpectedly | The session expired, reached its maximum age (`auth.session.max_age`, 90 d) or was revoked; `/auth/refresh` answers 401 for that token only. Two tabs of one browser refreshing at once do not cause this (see [Refresh](#refresh)); two separate clients sharing one refresh token do. |
| All sessions invalidated | A password reset, an admin disable, or "revoke all sessions" (each bumps `token_version` and revokes sessions; a self-serve password change does the same but keeps the current session). Refresh-token replay never does this. |
| OIDC user marked unverified | `email_verified` claim missing from the issuer; fix the Authentik mapping (`docs/install/authentik.md`) or set the trust flag. |
