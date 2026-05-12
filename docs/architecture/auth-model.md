# Auth Model

Phase 0 deliverable. Source of truth: [comic-reader-spec.md §17.1–§17.3, §9.6](../../comic-reader-spec.md).

## Modes

`COMIC_AUTH_MODE` ∈ {`oidc`, `local`, `both`}. The login screen reflects the mode.

| Mode | Use case |
|---|---|
| `oidc` | Homelab with Authentik/Keycloak/Dex. The most common deployment. |
| `local` | Single-user / small-family installs without an IdP. argon2id + optional TOTP. |
| `both` | Mixed — admin via OIDC, opportunistic local accounts (e.g., a guest reader). |

## Token shapes

### Access token (JWT, 24 h default)

```json
{
  "iss": "https://comics.example.com",
  "sub": "<user_uuid>",
  "aud": "comic-reader",
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

Algorithm: `EdDSA` (Ed25519). Server holds the keypair under `/data/secrets/jwt-ed25519.key`. The audience is hardcoded to `comic-reader`.

### Refresh token (random opaque, 30 d default)

- 32 bytes, base64url. Never inspectable.
- SHA-256 hash stored in `auth_sessions.refresh_token_hash`.
- Rotated on every use; old hash overwritten in the same transaction.
- `auth_sessions.ip` + `.user_agent` recorded on issue and refresh (M1) — used by `/me/sessions` (M5) and the audit log. Not enforced as a binding (proxy IPs change).
- `COMIC_JWT_REFRESH_TTL` controls the cookie lifetime; validated at boot to be `>= access TTL`.

## Cookie shape (web)

```
__Host-comic_session=<jwt>;     HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age=86400
__Host-comic_csrf=<csrf>;                 Secure; SameSite=Lax; Path=/; Max-Age=86400
__Secure-comic_refresh=<rt>;    HttpOnly; Secure; SameSite=Lax; Path=/;        Max-Age=2592000
```

`__Host-` requires `Path=/`, so the refresh cookie uses the slightly-weaker `__Secure-` prefix. The refresh path is `/` (not the narrower `/auth/refresh`) because the Next dev proxy rewrites `/api/auth/refresh` → `/auth/refresh` and browsers compare the cookie's `Path` against the original request URL — a narrower path made the cookie omit from the dev refresh call. SameSite=Lax + HttpOnly still cover the cross-site send + JS read protections; the path-narrowing was belt-and-suspenders that never paid off.

The CSRF cookie is intentionally **not** HttpOnly — JS reads it for the `X-CSRF-Token` header (double-submit). The session cookie is HttpOnly.

## Flows

### Local login

```
POST /auth/local/login {email, password, totp?}
   ↓
verify argon2id(password + pepper, hash)
   ↓
if user.totp_secret IS NOT NULL: verify(totp, secret)
   ↓
INSERT auth_sessions (refresh_token_hash) VALUES (sha256(rt))
   ↓
mint access (15m) → set __Host-comic_session
mint refresh (30d) → set __Host-comic_refresh
generate csrf       → set __Host-comic_csrf, return in body
```

### OIDC login (PKCE)

```
GET /auth/oidc/start
   ↓
generate code_verifier, store hash in __Host-comic_pkce (HttpOnly, 5min)
generate state, store in same cookie
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
POST /auth/refresh  (with __Host-comic_refresh cookie)
   ↓
look up auth_sessions WHERE refresh_token_hash = sha256(rt) AND revoked_at IS NULL AND expires_at > now()
   ↓
DELETE old row, INSERT new row with rotated refresh, in one transaction
   ↓
mint new access; set new cookies
```

If the lookup misses (token reuse, race, replay), all sessions for that user are revoked (`UPDATE auth_sessions SET revoked_at = now() WHERE user_id = ?`) and the user is forced through full re-auth. This is the standard refresh-token-reuse defense.

### CSRF (double-submit)

- Server sets `__Host-comic_csrf=<32B base64>` on `/auth/me`.
- Client reads cookie, includes value as `X-CSRF-Token` on POST/PUT/PATCH/DELETE.
- Server compares cookie value to header; mismatch = 403.
- Bearer-authenticated requests (mobile, API, OPDS-Bearer) skip CSRF entirely — there's no ambient credential to forge.

### WebSocket auth (one-shot ticket)

- `POST /ws/ticket` (cookie + CSRF) → server generates `ticket = base64url(rand 24)`, stores `(ticket → user_id)` in Redis with 30 s TTL, returns `{ ticket, expires_in: 30 }`.
- Client opens `wss://…/ws?ticket=<…>`. Server consumes the ticket atomically (Redis `GETDEL`) and accepts the upgrade if found and unexpired.
- Cookie is **not** re-checked at upgrade — the ticket is the credential.

### Logout

- `POST /auth/logout` revokes the current refresh row (`UPDATE … SET revoked_at = now()`), bumps `users.token_version` if "log out everywhere" was requested, clears all three cookies.

## Failure modes

| Symptom | Likely cause |
|---|---|
| Loop: cookie set, immediately rejected | `COMIC_PUBLIC_URL` doesn't match the host the cookie was set on (`__Host-` requires exact host). |
| OIDC callback 401 with valid token | Clock skew > 60 s; check NTP on the app host. |
| All sessions invalidated unexpectedly | Refresh-token reuse detected — likely two devices polling stale tokens. |
| OIDC user marked unverified | `email_verified` claim missing from the issuer; fix the Authentik mapping (`docs/install/authentik.md`) or set the trust flag. |
