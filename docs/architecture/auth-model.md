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
- Absolute age: `auth.session.max_age` (default `90d`, floor `1d`, `>=` access TTL, at most `3650d` like the access and refresh TTLs; **Admin → Auth → Tokens**). A session cannot be refreshed past `created_at + max age` however often it is used; `expires_at`, and the Max-Age of the refresh and CSRF cookies, are clamped to that bound at sign-in and on every refresh. `/me/sessions` does not list a session past it. Change it in the admin UI. `COMIC_SESSION_MAX_AGE` is also read, through the generic `COMIC_` env prefix, as the baseline while no row is saved; a saved row wins (see [runtime-configuration.md](../dev/runtime-configuration.md)).

Verified against the code at `5a1d2f64` on 2026-11-10.

## Cookie shape (web)

```
__Host-comic_session=<jwt>;     HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age=3600
__Host-comic_access_exp=<exp>.<ttl>;      Secure; SameSite=Lax; Path=/; Max-Age=3600
__Host-comic_csrf=<csrf>;                 Secure; SameSite=Lax; Path=/; Max-Age=2592000
__Host-comic_refresh=<rt>;      HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age=2592000
```

Max-Age values shown for the defaults. The session and access-expiry cookies live for the access TTL. The refresh cookie lives for the refresh TTL clamped to what is left of the session's absolute age, never below 1 s when the refresh was accepted. The CSRF cookie gets max(access TTL, that same clamped value) from one helper (`cookies::csrf_max_age`), so it never expires before the access cookie; handlers that don't load the session row (`/auth/me`, preferences, account updates) use the unclamped refresh TTL there.

`__Host-comic_access_exp` holds `<exp>.<ttl>`: the access token's `exp` (epoch seconds, server clock) and its lifetime in seconds. It is **not** HttpOnly, so the web client can see when the token lapses without reading the token. It holds no secret. It is set wherever the session cookie is set (local login and registration, refresh, the OIDC callback) and cleared with it (logout, account deletion).

All auth cookies are host-only. The refresh path is `/` (not the narrower `/auth/refresh`) because the Next dev proxy rewrites `/api/auth/refresh` → `/auth/refresh` and browsers compare the cookie's `Path` against the original request URL — a narrower path made the cookie omit from the dev refresh call.

The CSRF cookie is intentionally **not** HttpOnly — JS reads it for the `X-CSRF-Token` header (double-submit). The session cookie is HttpOnly. The CSRF cookie outlives the access cookie (see above): `POST /auth/refresh` is CSRF-checked, so it must still be there after the access cookie expired. It is not a credential on its own, and sign-out clears it.

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

A first OIDC login whose email belongs to a local (password) account is refused with 409 `auth.email_in_use` unless `auth.oidc.link_local_by_verified_email` is on. With it on, `auth/oidc.rs::callback` applies one rule:

- The ID token's `email_verified` is not the JSON boolean `true` (absent, even when the trust-unverified opt-in admits the login, or `false`; a string `"true"` fails ID-token parsing): no link. A pending local row gets 403 "verify your email first", any other row 409.
- A disabled local row is refused (403 `auth.disabled`) before anything is written.
- Otherwise the identity is linked and the account becomes **single sign-on only**, whatever the local row's own `email_verified` says (self-registration without mail sets it with no proof): the local password is dropped, `token_version` is bumped, every refresh session and app password is revoked, `email_verified` is set, and a pending row becomes active. The row update is conditional on the external id and `token_version` that were read; the update, the revocations and the audit row commit in one transaction, and a concurrent change answers a retryable 409 `auth.conflict`. The new session is minted only after the commit.

The `auth.oidc.auto_link` audit row records the account's `role`, `activated`, `password_dropped`, `sessions_revoked` and `app_passwords_revoked`, and the takeover is logged at WARN. A row an earlier build linked while keeping its local password loses that password the same way at its next single sign-on (`auth.oidc.local_password_dropped`); no supported path gives an OIDC-keyed account a password otherwise.

Because a linked account can no longer sign in with a password, keep a separate local admin account with a different email as a fallback for when the identity provider is unavailable.

Verified against the code at `59714b1f` on 2026-11-10.

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
  (refresh Max-Age = max(1 s, min(refresh TTL, created_at + max age - now));
   CSRF Max-Age = max(access TTL, that clamp))
```

The `WHERE` is evaluated under the row lock, so the rotation is atomic: of two concurrent refreshes carrying the same token, one updates the row and the other then finds the rotated hash, matches nothing and gets 401 `auth.invalid`. If nothing matches (an already-rotated token replayed, the loser of such a race, an expired, revoked or unknown token) the request gets 401 `auth.invalid` and only one more thing can happen: when the row exists, is not revoked and is past its maximum age, it is marked revoked (so `/me/sessions` stops listing it). The server cannot tell which user an unknown hash belonged to, so no other session is revoked. Refresh-token reuse detection is not implemented.

The rotation commits before the response leaves the server. If that response is lost (the connection drops, the tab is closed mid-request), the browser keeps the old refresh cookie, which is now dead, and the next refresh is refused: that session is over and the user signs in again. A short grace window accepting the previous token is a possible later change; it is not implemented. To cut off every device, revoke all sessions (`POST /me/sessions/revoke-all`, which also bumps `token_version`); a password change revokes every session except the current one.

**Proactive refresh.** Signed-in pages mount a scheduler ([`web/lib/api/session-refresh.ts`](../../web/lib/api/session-refresh.ts), mounted once by the root layout for a signed-in request, inert on the auth pages). It reads `__Host-comic_access_exp` and refreshes at about 80 % of the remaining lifetime, never later than 30 s before expiry (with the 1 h default: about 48 min after each renewal). It re-arms after every refresh, its own or another tab's, when the page becomes visible again and when the browser comes back online; a page that becomes visible with the token near or past expiry refreshes at once. Without the CSRF cookie (signed out) it arms nothing. The expiry is on the server's clock, so every comparison uses the client clock plus an offset learned from the `Date` header of the refresh response and of any response through `apiFetch` (zero until one arrives). Whatever the clocks say, a proactive refresh (the scheduler's or `ensureFreshSession()`'s) never starts within max(60 s, 10 % of the token lifetime from the cookie) of the previous one in that tab; a refresh with no answer is retried after 30 s, doubling to 10 min, reset when the page becomes visible or the browser comes back online. Requests the 401 retry cannot cover call `ensureFreshSession()` first, which resolves at once when the token is fresh and otherwise waits for a refresh (promptly when offline or on a network failure): the reader's page-image retry and Tap to retry, a prefetch batch near expiry, and one retry of an offline-download asset that got 401.

**Two tabs.** Tabs share cookies, so the tab that loses a refresh race already holds the winner's new tokens. The web client ([`web/lib/api/auth-refresh.ts`](../../web/lib/api/auth-refresh.ts)) serializes refreshes across tabs with the Web Locks API (`folio:auth-refresh`). Whether another tab refreshed is read from the access-expiry cookie: it counts only when the cookie is present, later than the value this tab saw before it tried, and in the future. A missing cookie never counts (sign-out clears it), and a changed CSRF cookie alone means nothing. Inside the lock, a tab that sees the moved expiry skips its own refresh. Without Web Locks, a 401 from `/auth/refresh` counts as a lost race when the expiry moved, checked again after 300 ms; either way the tab retries its original request once with the new cookies. Only when the expiry did not move is the session treated as gone, and the client then expires the CSRF and access-expiry cookies itself so nothing keeps retrying a dead session. A refresh that times out (15 s), fails on the network, gets a 5xx or gets a 403 (a CSRF failure, which says nothing about the session) is "no answer": nothing is signed out and no private state is cleared.

**Full page loads.** Server-rendered pages cannot refresh. A page load whose access cookie expired is redirected to `/sign-in?next=…`; when a CSRF cookie is present (a session may exist) and the URL does not carry `signed_out=1`, the sign-in page first shows "Signing you in…" (rendered on the server, so the form does not flash; the card carries a `<noscript>` link and a "Sign in instead" link that CSS reveals after 4 s, or at once with reduced motion, both to `/sign-in?resume=0`, which renders the form without resuming and without a banner), calls the same refresh and, on success, returns to `next`. Loop guard: every attempt, successful or not, is stamped in sessionStorage and the page does not try again within 30 s; without sessionStorage it tries at most once per page load. A refused refresh also expires the CSRF cookie, so the next visit does not try at all.

**What can still sign a user out:** sign-out or "sign out everywhere"; a session past its maximum age, expired (unused for the refresh TTL) or revoked; an account disabled; a password change on another session; a lost refresh response (above); two separate clients sharing one refresh token; and a browser that stays asleep or offline past the refresh lifetime.

Verified against the code at `5a1d2f64` on 2026-11-10.

### CSRF (double-submit)

- A new `__Host-comic_csrf=<32B base64>` value is minted at sign-in, registration, refresh and the single sign-on callback. `/auth/me` mints one only when the cookie is absent; preferences and account updates keep the caller's value and only extend its lifetime, so no request already in flight is failed by a rotation.
- Client reads cookie, includes value as `X-CSRF-Token` on POST/PUT/PATCH/DELETE.
- Server compares cookie value to header; mismatch = 403.
- Bearer-authenticated requests (mobile, API, OPDS-Bearer) skip CSRF entirely — there's no ambient credential to forge.
- The sign-in POSTs that are exempt from the token check because no session exists yet (`/auth/local/login`, `/register`, `/request-password-reset`, `/reset-password`, `/resend-verification`) get the cross-site guard below instead.

### Cross-site sign-in posts (login CSRF)

Without a guard, another site could post the sign-in form with credentials it controls and sign the visitor into the attacker's account. For the five routes above, `auth::csrf` applies [`auth::origin::cross_site_reason`](../../crates/server/src/auth/origin.rs) before the handler runs:

1. When the request carries `Sec-Fetch-Site`, that header alone decides: `same-origin` and `none` proceed without looking at `Origin`; `cross-site`, `same-site` and any other value are refused. It is a forbidden header name, set by the browser and never by a page, so it is trusted over `Origin`. `same-site` is refused deliberately: a sibling subdomain (another app on the same registrable domain) must not sign visitors in either, and nothing in Folio posts these forms from another subdomain.
2. Only without `Sec-Fetch-Site` (older browsers) does `Origin` decide. It proceeds when it equals the configured public origin (`public_url`, scheme + host + port); or its host and port equal the request's own `Host` header (the HTTP/2 authority when there is no `Host`; a missing port means the `Origin` scheme's default), which keeps same-origin posts working over a LAN address or another host name; or, when the peer is a trusted proxy (`COMIC_TRUSTED_PROXIES`), its host and port equal the well-formed `X-Forwarded-Host` that proxy set. An untrusted peer's `X-Forwarded-Host` is never read. `Origin: null` is refused.
3. A request with neither header (native clients) proceeds.

A refused form navigation (a `application/x-www-form-urlencoded` or `multipart/form-data` body, or `Sec-Fetch-Mode: navigate`) gets a 303 back to the page the form lives on with `error=auth.csrf`: `/sign-in` for login, register and resend-verification, and `/forgot-password` for request-password-reset and reset-password (nothing from a refused request's body is echoed back, so the reset token is not carried over and the user asks for a fresh link). Those pages show "That request could not be verified. Reload the page and try again." Any other refused request gets the 403 `auth.csrf` envelope. The answer is the same whichever check failed (the reason is logged at INFO only), and a refused request never reaches the handler, so it is not counted by the failed-auth lockout. The OIDC callback is a `GET` (a cross-site redirect by design, gated by its state cookie) and is not affected, nor are the `GET` verify-email link and `/csp-report`. The `/api`-prefixed mounts of these routes are not exempt at all and need the CSRF token.

**Referrer-Policy on the auth pages.** `/sign-in`, `/forgot-password` and `/reset-password` are served with `Referrer-Policy: same-origin` (every other response: `strict-origin-when-cross-origin`). No `Referer` goes to another site, so credentials that landed in the URL or a reset token cannot leak to the next site visited. It was `no-referrer` before; under that policy browsers send `Origin: null` on the pages' own form posts, which rule 2 refuses for browsers without `Sec-Fetch-Site`. The server does not log `Referer`.

### Sign-in check order

`POST /auth/local/login`: (1) refuse with 429 if the client IP or the submitted email is locked out; (2) verify the password, for every account: against the stored hash, or, for an unknown address or an account without a local password, against a dummy hash at the same cost, so all paths pay the same argon2 work; (3) a wrong password, unknown address or missing hash records a failure on both lockout axes and answers the generic 401 `auth.invalid`, whatever the account's state; (4) only after a correct password: a disabled account gets 403 `auth.disabled` and an unverified one 403 `auth.email_unverified`, and both still record a failure on both axes, as before; (5) otherwise the session is issued. The no-JavaScript form gets the same outcomes as a redirect to `/sign-in?error=<code>`.

Registration still answers 409 for an address that has an account; that is an accepted trade-off of open registration (the email-change route reveals the same).

Every argon2 hash and verify reached from an async handler (sign-in and its dummy verify, registration, password change, password reset, account deletion and the email/password re-checks through `failed_auth::verify_password_counted`, admin-created users) runs on tokio's blocking pool through `auth::password::{hash_blocking, verify_rotating_blocking, dummy_verify_blocking}`; app-password verification is handled in `auth::app_password` separately. A process-wide semaphore admits at most `password::hash_permits()` of these at once (the available parallelism, at least 2: the bound hashing had when it ran inline on the async workers); further callers wait in line, with no timeout. A failed hashing task (a panic or cancellation) answers 500 `internal` and is not counted as a wrong password; an unparsable stored hash counts as a non-match.

Verified against the code at `9492ac1b` on 2026-11-10.

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
(`age_rating_max`) and a **Hide unrated** switch (below), both set per
library from `/admin/users/{id}` → *Library access*. The cap is enforced
server-side on every read surface; the web app never filters client-side.

The picker lists "No cap" first, then every rung under a plain group
heading in ladder order (`AGE_RATING_GROUPS` in `web/lib/age-rating.ts`:
Young children = Early Childhood; All ages = Everyone, G; Older children
= Everyone 10+, PG, Kids to Adults; Teens = Teen, MA15+; Mature = Mature
17+, M; Adults only = R18+, Adults Only 18+, X18+), each with a one-line
description of who it suits. The web copy of the ladder is pinned to the
server's by `tests/library_age_ratings.rs`. Each library row shows what
the selection does before saving: how many series the cap (and switch)
hide and how many unrated series stay shown or are hidden. The figures
come from `GET /admin/libraries/age-ratings` (admin, read-only): per
library, the active series at each rung (aliases counted on their rung,
after the same normalisation as the cap) and the unrated count; the web
does the arithmetic for whatever is selected. They count **series** only:
an issue rated above the cap inside a visible series is hidden too and is
not counted. The same test checks, for every cap with and without
`hide_unrated`, that the hidden count equals what `GET /api/series` leaves
out for a reader holding that grant.

### Ladder

The cap is a rung of the ComicInfo `AgeRating` vocabulary, ordered
youngest-audience-first (`crates/server/src/library/age_rating.rs`):

```
Early Childhood < Everyone < G < Everyone 10+ < PG < Kids to Adults < Teen
  < MA15+ < Mature 17+ < M < R18+ < Adults Only 18+ < X18+
```

Comparison is case-insensitive and ignores surrounding ASCII whitespace
(space, tab, line feed, carriage return, form feed, vertical tab; the
Rust `RATING_TRIM` and the SQL `btrim` strip the same set). The admin API
validates a cap against the ladder (422 otherwise) and stores the
canonical spelling.

**Aliases.** Metron rates with its own words. `Everyone` and `Teen` are
already rungs; the others rank as a rung wherever ratings are compared
(cap checks, the SQL lists, the series derivation), while the stored
text is left as it is:

| Metron word | ranks as |
|---|---|
| `Teen Plus` | `MA15+` (the rung just above `Teen`) |
| `Mature` | `Mature 17+` |
| `Adult` | `Adults Only 18+` |
| `Explicit` | `X18+` |

The vocabularies that reach `age_rating`: ComicInfo uses the ladder;
MetronInfo and the Metron API use the words above; the GCD provider maps
its ratings onto ladder words; ComicVine carries no rating; `series.json`
is free text and ranks only when it uses a ladder word or an alias. An
alias is not a valid cap.

Edits validate the value: the issue PATCH, the bulk-metadata PATCH and
the series PATCH accept a ladder rung, an alias, or blank / `null`
(clear), and answer 422 with the field error "not a recognised age
rating" otherwise. The issue and bulk edits also accept `Unknown` /
`Rating Pending`; the series PATCH refuses them with a 422 that says to
clear the rating instead (a pinned "no rating" would leave the series
unrated for good and override its derived rating). Values already
stored are not re-checked.

### How a series is rated

A series' `age_rating` is derived: it is the highest-ranked rating among
the series' active, not-removed issues **and the rating its folder's
`series.json` declares** (`library::series_rating`), stored in the
ladder's canonical spelling. The declared rating is a floor: the scanner
records it in `series.declared_age_rating` on every scan that reads the
folder, so a `series.json` that says `Mature 17+` keeps the series at
least `Mature 17+` even when its issues are unrated or rated lower, and
an issue rated above the declared value raises the series. A
`series.json` that is removed, or loses its rating, clears the floor at
the next scan only when the file is really gone (its lookup says
not-found while the folder is still a directory). One that exists but
cannot be parsed, a vanished folder or a dropped share, and any other
lookup error leave the floor unchanged. Every library scan reads every
planned folder's `series.json`, folders whose files are unchanged
included, and re-derives the series rating each time. The upgrade
migration never lowers a rating and holds a stored rating that ranks at
least as high as every issue as a provisional floor until that first
scan. Each recompute locks the series row before it derives, so one that
queued behind another writer derives from that writer's committed
issues. A
rating that ranks nowhere (off-ladder text) never raises the result; a
series with nothing that ranks is unrated (`NULL`). The value is
recomputed by the scanner's per-series rollup, when the declared rating
changes, and whenever an issue's rating or a series' set of active
issues changes (issue edits, bulk metadata, a provider apply,
soft-removal and restore).

A rating set on the series by a person (series edit) is kept: a
`field_provenance` row for the series field `age_rating` whose `set_by`
is not a file-tier code protects it (`metadata::writers::is_file_tier_set_by`,
user > provider > file; no metadata provider writes a series rating
today). An admin holds a series' rating by setting it on the series.
Clearing the rating in the series edit (`age_rating: null`) removes that
`user` pin and re-derives the rating in the same transaction, so the
series returns to its derived value.

### What a capped user sees

- A **series** is visible when `series.age_rating` ranks at or below the
  cap.
- An **issue** is visible when its series is visible AND its own
  `age_rating` ranks at or below the cap. An issue with no rating of its
  own (or one not on the ladder) follows its series. So an issue rated
  above its series is hidden on its own, and every issue of a hidden
  series is hidden, whatever its own rating: it does not open by id, and
  it is absent from issue lists, search, rails, feeds and entity
  surfaces.
- **Unrated content is shown** by default (decision D6, 2026-09-29):
  `NULL`, empty, `Unknown`, `Rating Pending` and any string not on the
  ladder all pass. Only rows whose rating is a known rung *above* the cap
  are hidden, so the SQL predicate is `rating IS NULL OR
  lower(btrim(rating, <trim set>)) NOT IN (<rungs and aliases above the
  cap>)`, never an allow-list.
- **Hide unrated** (`library_user_access.hide_unrated`, per grant, off by
  default): when on, a series with no ladder rating is hidden from that
  user exactly as if it ranked above the cap, and with it all its issues.
  An issue with no rating of its own inside a rated series follows the
  series and is not unrated. The option works without a cap: then it
  hides only unrated content. The admin API field is `hide_unrated` on
  `POST /admin/users/{id}/library-access` (a map of library id to
  boolean) and on each grant in the user detail; the admin screen sets it
  with the *Hide unrated* switch beside the cap. Admins are unaffected.

### Surfaces enforced

The predicate is carried on `library::access::VisibleLibraries` (built
once per request by `access::for_user`, with each grant's cap and
`hide_unrated`) and applied in three shapes — `series_filter()` /
`issue_filter()` / `issue_cap_condition()` for sea-orm queries,
`raw_cap_clause()` (series rows) and `issue_cap_sql_joined()` /
`issue_cap_sql()` (issue rows) for hand-written SQL, and `series_ok()` /
`issue_ok()` / `access::issue_visible()` for single rows (lists go
through `access::filter_issues`, one parent-rating query per batch). Every
issue predicate includes the series condition. A series rating that could
not be read (lookup failure, series row missing) counts as hidden wherever
the grant hides anything (`VisibleLibraries::issue_ok_lookup`). A caller whose grants hide
nothing gets no predicate at all, so its SQL is unchanged. Every surface
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
sits in a non-removed series of a granted library and is visible under
the issue rule above (`VisibleLibraries::issue_cap_sql_joined`, the same
predicate every issue surface uses): entity surfaces never name a series
the caller cannot open, even through an issue rated below it.

- **Issue junctions** (`issue_credits`, `issue_characters`,
  `issue_teams`, `issue_locations`, `issue_arcs`, `issue_genres`,
  `issue_tags`): a row counts when its issue counts.
- **Series rollups** (`series_credits`, `series_characters`,
  `series_teams`, `series_locations`, `series_genres`, `series_tags`):
  the scanner rebuilds these as the union of every issue in the series,
  hidden ones included. For a caller whose grant hides anything (a cap
  that hides a rung, or `hide_unrated`; `VisibleLibraries::hides_any`)
  they are not read; the issue junction
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
Admins and callers whose grants hide nothing keep the rollup queries
unchanged.

### Hidden and missing answer alike

Two cases are kept apart.

**Hidden by access.** To a caller who may not see an object (a library
they hold no grant on, a series or issue above their cap or hidden by
their grant's *hide unrated* option, another user's
personal list, view, collection, batch or marker), the object does not
exist: every read and write route answers exactly as it does for an id
that was never issued, with the same status, error code and body. A 403
is kept only for an action the caller may not take on something they can
see (a reader editing a shared reading list or a system view, a reader
calling an admin-only route). Admins have no grant or cap limits.

**Soft-removed content** (the file went missing; it may be restored, and
its history is kept) is not secret:

- A removed series answers not-found to non-admins on every route
  addressed through its slug or id, exactly as its page does
  (`access::series_page_ok` / `series_page_visible`), and so do the issue
  routes addressed through the series slug (`/series/{s}/issues/{i}` and
  its sub-routes, `access::issue_page_allowed` /
  `issue_route_visible`). Admins still open it to restore or confirm the
  removal. Routes addressed by an issue id alone (page bytes, thumbnails,
  covers, OCR, the reader's marker overlay, page overrides, the Komga
  book, the issue and marker permalinks) keep serving its issues as
  history.
- A removed issue in a live series keeps its reads: its detail page,
  cached thumbnails, the reader-side data for it (the reader's marker
  overlay, page overrides, the issue-level metadata and external-id
  reads) and the reading log, which lists such issues on purpose.
- Nobody, admins included, makes new personal writes on removed content:
  no new progress through any protocol, marker, marker edit, rating,
  closing note, reading session, page override, collection entry, reader
  preference or completion override. These key on `access::visible_issue`
  / `access::visible_series` (the `visible_issues_select` /
  `visible_series_select` predicates). Deleting one's own rows stays
  possible (`DELETE` of a marker, of page overrides, of reader
  preferences, and clearing a completion override). Rows written before
  the removal stay and come back with a restore.
- Admin curation of removed rows is deliberate and stays allowed: issue
  and series `PATCH`, external-id edits, metadata apply and series
  relationships are how a removed row is prepared for a restore.
- Two jobs a reader can start keep their earlier behaviour on a removed
  issue in a live series: a metadata search and OCR.

A caller's own markers follow the two cases. On content hidden by access
the marker list, count, search, tag index and permalink leave them out;
reading or editing one by id answers like an unknown marker; deleting
one by id still works. On removed content the marker surfaces keep
their earlier behaviour: the list, count, search and tags leave
markers on removed issues out, and the reader's overlay for the issue
still shows them.

The notes export (`/me/markers/export`) carries every marker and closing
note the user owns. Removed content keeps its series and issue identity,
`available: false`, no jump URL. Content hidden by access names nothing:
its markers and notes sit in one anonymous "No longer available" group
with only the fields the user wrote. The account export (`/me/export`)
blanks the names (series name, year, issue number, library slug) of
content hidden by access and keeps ids and content hashes for a
re-import; removed content keeps its names.

A CBL saved view may reference only a reading list the caller owns or a
shared one; any other list id is refused like a nonexistent one. A view
row written before that check that points at someone else's list reads
like a view whose list is gone: no list id, no year range, no covers, no
Up Next walk, no "appears in" entry, no OPDS pin, and the list itself
answers 404.

A metadata batch is readable by admins and by its creator only. A
creator who has since lost the admin role still sees every child of
their own batch, including children named past a cap set later; such a
batch can only have been created while they were an admin.

Routes whose answer changed (reader = non-admin; "hidden" = outside the
grant, above the cap, or unrated under *hide unrated*; an issue is
hidden also when its series is):

| Route | Case | Before | After |
|---|---|---|---|
| `POST /me/progress/bulk` | issue in a soft-removed series | written | counted in `not_found` (`forbidden` is always 0) |
| `POST /me/progress/series-bulk` | soft-removed series | written | counted in `not_found_series` |
| `POST /series/{slug}/progress`, `…/progress-matching` | soft-removed series | 200 | 404 `not_found` |
| `POST /progress`, OPDS `PUT …/progress`, KOReader sync `PUT`, OPDS Progression `PUT`, Komga `PATCH` | soft-removed issue or series | written | the route's own unknown-id answer |
| `POST /me/markers`, `POST /me/markers/restore` | soft-removed issue or series | written | 404 / counted in `skipped` |
| `PATCH /me/markers/{id}` | another user's marker; own marker on a hidden or removed issue | 403 `forbidden`; 200 | 404 `not_found` (on a removed issue this is a refused write: the reader's overlay still shows the marker) |
| `DELETE /me/markers/{id}` | another user's marker | 403 `forbidden` | 404 `not_found` |
| `GET /me/markers/export` | own marker or closing note on a hidden issue | exported with names | exported in the anonymous group |
| `GET /me/export` | rows on a hidden issue or series | names | ids and content hash only |
| `GET /me/series/{id}/reader-prefs` | hidden series, removed series for a reader; unknown id | 200 | 404 `not_found` |
| `PUT /me/series/{id}/reader-prefs` | hidden series; removed series, any role | 200 | 404 `not_found` |
| `PUT /me/series/{id}/completion-prefs` | removed series, any role, setting an override | 200 | 404 `not_found` |
| `GET /series/{slug}/external-ids`, `…/provider-ranges`, `…/provider-coverage`, `…/metadata/{search,candidates,coverage-hints,status,lookup,proposed-diff,composite-diff}` | hidden series | 403 `auth.forbidden` | 404 `not_found` |
| `…/metadata/overview`, `…/metadata/issues`, `/series/{slug}/collection`, `…/issues`, `…/resume`, `…/similar`, `…/relationships`, `…/appearances`, `…/same-universe` | soft-removed series (reader) | 200 | 404 `not_found` |
| `GET /series/{slug}/issues/{issue}/external-ids` and the issue-level metadata routes | hidden series | 403 `auth.forbidden` | 404 `issue.not_found` |
| the same | soft-removed series (reader) | 200 | 404 `issue.not_found` |
| `GET /series/{s}/issues/{i}`, `…/metadata-overview`, `…/next`, `…/prev`, `…/health-issues`, `…/appearances` | soft-removed series (reader) | 200 | 404 `not_found` |
| `GET /metadata/batch/{id}` | another user's batch | 403 `forbidden`, or 200 with a library grant | 404 `not_found` |
| `PUT /series/{slug}/rating` | soft-removed series | 200 | 404 `not_found` |
| `PUT /series/{s}/issues/{i}/rating`, `…/note` | soft-removed issue or series | 200 | 404 `not_found` |
| `PUT /me/issues/{id}/page-overrides`, `POST /me/reading-sessions` | soft-removed issue or series | 200 | 404 `not_found` |
| `DELETE /me/issues/{id}/page-overrides` | hidden or unknown issue | 404 | 204 (only the caller's own row is removed) |
| Every `/series/{s}/issues/{i}/…` route | unknown series slug | 404 "series not found" | 404 "issue not found" (as for a hidden series) |
| `POST /me/saved-views` (`kind: cbl`) | another user's list; unknown list | 500 on an unknown id | 422 `validation`, field `cbl_list_id`, for both |
| `PATCH`, `DELETE /me/saved-views/{id}` and its pin, sidebar, icon, results and issue-results routes | another user's view | 403 `forbidden` | 404 `not_found` (a system view stays 403) |
| `/me/collections/{id}` and every sub-route | another user's collection | 403 `forbidden` | 404 `not_found` |
| `/me/cbl-lists/{id}` and every sub-route | another user's list | 403 `forbidden` | 404 `not_found` (a reader writing a shared list stays 403) |
| `GET /opds/pse/…` page stream | hidden issue | 403 `library_access_denied` | 404 `not_found` |
| `GET` filter-option pickers with `?library=` | hidden library | 403 `forbidden` | 404 `not_found` |
| `GET /me/saved-views`, `/me/view-covers`, appearances, starter lists, Up Next / Prev Up `?cbl=`, OPDS pins | CBL view row pointing at another user's list | list id, year range, covers, list name, walk, pin | as if the list were gone |

Regression suite: `crates/server/tests/denied_and_missing_alike.rs`
compares each hidden request with the same request on a random unknown
id, byte for byte, for a reader without a grant and a capped reader,
and checks the soft-removed rules for a reader with a grant.

Regression suite: `crates/server/tests/age_rating_cap.rs` (a `Teen`-capped
user against `Teen` / `Mature 17+` / unrated series, an issue-level
override, a series rated above its issues, the derived series rating with
its `series.json` floor, the upgrade rule, Metron's words and
`hide_unrated`) and the unit tests in `library/age_rating.rs` /
`library/access.rs` / `library/series_rating.rs`.

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
