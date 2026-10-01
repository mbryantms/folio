# Folio Security Audit — 2026-05-15

Synthesized from four parallel investigations covering auth/session/CSRF,
authZ/admin/rate-limit, input/SQL/path/upload/SSRF, and
secrets/logging/CORS. The most actionable claims were verified against
the code before finalizing; several agent findings turned out to be
false positives and are noted at the bottom.

---

## Status per finding

Last reviewed **2026-10-01** (WP-6.3, residual security items). The
original write-up below is kept as-is for traceability; this table is the
source of truth for what is still open. "Fixed (earlier)" items were
re-verified against the code during WP-6.3.

| Finding | Severity | Status | Where / regression guard |
|---|---|---|---|
| H-1 SSRF via `POST /me/cbl-lists` (URL kind) | High | **Fixed (earlier)** — 1.0 Phase B; https-only, DNS-vetted + address-pinned fetch, per-hop redirect re-validation (`util::ssrf`) | `crates/server/src/util/ssrf.rs` unit tests; `tests/cbl_lists.rs` |
| H-2 `Config` derives `Debug` with plaintext secrets | High | **Fixed (earlier)** — hand-written masking `Debug` impl | `crates/server/src/config.rs` (`impl Debug for Config`) |
| H-3.1 failed-auth lockout keyed only by identifier | High | **Fixed (earlier)** — IP axis + email axis (`auth::failed_auth`) | `tests/rate_limits.rs` (`failed_auth_lockout_*`, `email_axis_lockout_*`) |
| H-3.2 OPDS shared rate bucket | High | **Fixed (WP-6.3)** — split into `opds_catalog` (30/min, burst 60) and `opds_stream` (downloads, PSE pages, progress/KOReader sync; 120/min, burst 120) | `tests/rate_limits.rs::opds_catalog_exhaustion_does_not_throttle_the_stream_bucket` |
| M-1 no explicit `CorsLayer` | Medium→Low | **Fixed (WP-6.3)** — `app::deny_all_cors()` (no origin/credentials/methods granted; preflights answered with no `Access-Control-Allow-*`) | `tests/http_layers.rs::cors_*` |
| M-2 `/auth/config` exposes OIDC issuer | Medium | **Fixed (earlier)** — 1.0 Phase B4 (`PublicAuthConfigView` carries booleans only); WP-6.3 re-verified and added a whole-body "issuer string never appears" assertion | `tests/admin_auth_edit.rs`, `tests/oidc.rs::public_auth_config_advertises_oidc_when_configured` |
| M-3 secret file permissions not re-validated | Medium | **Fixed (earlier)** — `validate_existing_secret_file` refuses symlinks and any group/other mode bit (also applies to the new `pepper.previous`) | `secrets::tests::existing_group_readable_secret_file_is_rejected` |
| M-4 OIDC state cookie path narrower than other cookies | Medium | **Fixed (earlier)** — state cookie is `Path=/` | `crates/server/src/auth/oidc.rs` |
| M-5 no per-user CBL import quota | Medium | **Fixed (WP-6.3)** — 20 import-class ops (create upload/URL/catalog, manual refresh/check) per user per hour (Redis, `429` + `Retry-After`), plus a standing cap of 50 server-fetched (`url`/`catalog`) lists per user (`422 cbl.remote_list_limit`); admins exempt (`cbl::quota`) | `tests/cbl_lists.rs::import_quota_*`, `remote_list_cap_*` |
| L-1 no pepper rotation path | Low | **Fixed (WP-6.3)** — dual-pepper verify-and-rehash via `secrets/pepper.previous`; runbook in [`docs/install/secrets-backup.md`](../install/secrets-backup.md#rotating-the-pepper) | `tests/pepper_rotation.rs`, `auth::password::tests::rotating_verify_*`, `secrets::tests::moving_pepper_aside_*` |
| L-2 email-token verifier accepts expiry up to 30 days out | Low | **Open** — not in WP-6.3 scope. Tokens are single-use and HMAC-signed; the bound is effectively a max-TTL check, not a clock-skew window. | — |
| L-3 CBL multipart upload accepts any `Content-Type` | Low | **Fixed (earlier)** — `is_allowed_cbl_content_type` (`415` before the body is read) | `tests/cbl_lists.rs::upload_rejects_obvious_non_xml_content_type` |
| L-4 `LIMIT` via `format!` interpolation | Low | **Fixed (WP-6.3)** — every raw-SQL `LIMIT`/`OFFSET` now a bound parameter (`people`, `creators`, `filter_options`, `entity_pages`, `reading_sessions`, `admin_activity`) | `tests/sql_limit_binding.rs` (source scan) |

Findings from the 2026-09 product audit (`docs/product-audit.md` §4, SE-3 /
SE-6) that belong to the same remediation pass:

| Finding | Status | Where / regression guard |
|---|---|---|
| SE-3a `override_user_edits` gated by inline `user.role != "admin"` at five sites | **Fixed (WP-6.3)** — `api::extractors::AdminGatedOverride<T>` runs `RequireAdmin` whenever the body sets the flag (403 `auth.permission_denied` before the handler body); all five handlers (`apply_series`, `apply_issue`, `composite_apply_series`, `composite_apply_issue`, `batch_apply`) take it | `tests/metadata_search_api.rs::apply_series_403_when_override_user_edits_requested_by_non_admin`, `tests/metadata_composite.rs::composite_apply_override_is_admin_gated_structurally` |
| SE-3b composite apply never audited as `_force`; dashboard counter excluded it | **Fixed (WP-6.3)** — `admin.{series,issue}.metadata_composite_apply_force` when the override is set; `applies_last_7_days` counts `admin_metadata::MANUAL_APPLY_ACTIONS` (single + composite, plain + force) | `tests/metadata_composite.rs::composite_force_apply_audits_as_force_and_counts_on_the_dashboard` |
| SE-6 provider cover fetch allowed `http://`, no content check, MIME from URL extension | **Fixed (WP-6.3)** — `metadata::writers::fetch_cover_bytes` (https-only incl. redirect hops, magic-sniffed); `apply_cover` refuses non-image bytes and names the file from the sniffed type; the cover endpoint serves the sniffed MIME with `nosniff` and refuses a stored non-image | `tests/issue_cover_apply.rs` (`non_image_cover_bytes_*`, `stored_cover_extension_*`, `cover_fetch_refuses_plain_http_*`, `cover_endpoint_serves_sniffed_mime_*`) |

---

## Release Blocker

**One finding requires fixing before any multi-tenant deployment.**

### H-1. Authenticated SSRF via `POST /me/cbl-lists` (URL kind)

[crates/server/src/api/cbl_lists.rs:875-897](../../crates/server/src/api/cbl_lists.rs#L875)

`create_from_url` accepts any URL from an authenticated user and passes
it straight to a default `reqwest::Client` with **no scheme/host
validation and default redirect-following**. Any registered user can:

- Probe `http://169.254.169.254/...` (AWS/GCP/Azure instance metadata)
  — credential exfiltration on cloud deploys.
- Hit `http://localhost:5432/` / `:6380/` (Postgres, Redis) and infer
  service presence from error timing.
- Enumerate the LAN (`10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`,
  `169.254.0.0/16`).
- Bypass naive scheme checks via redirect (default policy follows up
  to 10 hops).

Exfiltration is capped by the 4 MiB body limit + XML parse-or-fail,
but reconnaissance is fully open and timing oracles work.

**Fix:** Add a URL guard before `.send()` that (a) requires `https://`,
(b) resolves the host and rejects loopback/private/link-local IPs
(use `std::net::IpAddr::is_loopback`/`is_private` plus a CIDR check for
`169.254.0.0/16`), (c) sets `redirect::Policy::limited(2)` and
re-validates each redirect target. Apply the same guard to the periodic
refresh path. For belt-and-suspenders, also add a per-user import rate
limit (e.g., 10/hour).

---

## High

### H-2. `Config` derives `Debug`; sensitive fields are plain `String`

[crates/server/src/config.rs:46](../../crates/server/src/config.rs#L46)
(struct), fields at `:48, :51, :75, :151`.

The startup banner currently logs only 4 safe fields, so this is
latent — but any future `tracing::debug!("{cfg:?}")` or panic that
captures `Config` would dump `database_url` (Postgres password),
`redis_url`, `oidc_client_secret`, and `smtp_password` to
stdout/journalctl. Master keys are properly `Zeroizing` but these
stay plaintext for the life of the process.

**Fix:** Hand-write a `Debug` impl that masks `oidc_client_secret`,
`smtp_password`, `database_url`, `redis_url` (e.g., `"***"`). Or use
`secrecy::SecretString` for the two credentials and a wrapper newtype
for the URLs. Cheap; high payoff against operator misconfiguration.

### H-3. Rate-limit placement vs. failed-auth lockout

[crates/server/src/middleware/rate_limit.rs](../../crates/server/src/middleware/rate_limit.rs) +
[crates/server/src/auth/failed_auth.rs](../../crates/server/src/auth/failed_auth.rs)

Per-route governor buckets are correctly defined (LOGIN 5/min, etc.).
Two concerns to verify on the next pass:

1. The failed-auth Redis lockout is a separate mechanism (counts 401s
   per identifier, locks for 15 min after N failures). A slow
   brute-forcer pacing requests under the 5/min governor still trips
   the Redis counter, which is correct. But the Redis lockout is
   keyed by *username/email*, not IP — so an attacker spraying many
   usernames from one IP bypasses both. Add an IP-keyed counter as a
   second axis.
2. The `OPDS` bucket (60/min, global) treats catalog enumeration and
   per-issue page downloads as a single budget. A scraper exhausts the
   budget against `timeline`/`search` and then legitimate page reads
   get throttled. Split: enumeration 10–20/min, downloads 60/min.

---

## Medium

### M-1. No explicit `CorsLayer`

[crates/server/src/app.rs](../../crates/server/src/app.rs) — confirmed:
no CORS middleware anywhere.

Browser same-origin enforcement is the only cross-origin barrier
today. That's adequate for normal browsers, but defense-in-depth and
(more importantly) explicit intent matters: a future maintainer
adding "just a quick test endpoint" without CORS awareness inherits
no guardrails.

**Fix:** Mount a restrictive `tower_http::cors::CorsLayer` with no
`allow_origin` / no `allow_credentials` so any cross-origin preflight
is rejected. Document why it's present.

> **Updated 2026-05-16 (rust-public-origin v0.2 cutover):** the
> attack surface here is narrower than the original write-up
> suggested. With the Rust binary now the public origin and Next.js
> behind it as an internal upstream, the entire app — HTML and API —
> shares a single same-origin envelope at the public host. There is
> no longer a class of "client app on origin A talking to API on
> origin B" cross-origin flow we need a CORS policy to gate. The
> recommendation stands as defense-in-depth + explicit-intent, but
> downgrade urgency from Medium to Low when planning remediation.

### M-2. `GET /auth/config` exposes OIDC issuer URL to anonymous callers

[crates/server/src/api/auth_config.rs](../../crates/server/src/api/auth_config.rs)
(`PublicAuthConfigView`).

The public auth-config endpoint returns the OIDC issuer URL to
*anyone*. Issuer URLs frequently reveal internal topology
(`https://auth.internal.company.com`) or specific IdP vendors usable
for targeted phishing.

**Fix:** Return only `{auth_mode, local_enabled, oidc_enabled}` to
anonymous callers. Reserve `issuer` for authenticated `GET /me` or
`RequireAdmin /admin/auth`.

### M-3. `settings-encryption.key` permissions not re-validated on load

[crates/server/src/secrets.rs:40-90](../../crates/server/src/secrets.rs#L40)

The key is *created* with mode 0600. On subsequent boots, the loader
reads it without checking mode. If someone runs `chmod 644
secrets/settings-encryption.key` (Docker image misconfiguration, ops
typo, restore-from-backup with bad umask), the file becomes
world-readable and the server starts happily.

**Fix:** In `load_or_generate_bytes`, `stat` the file after open and
bail with a clear error if `mode & 0o077 != 0`. Same check for the
pepper file.

### M-4. OIDC state cookie `Path=/auth/oidc/callback` is narrower than other auth cookies

[crates/server/src/auth/oidc.rs:297](../../crates/server/src/auth/oidc.rs#L297)

Cookie is HttpOnly, so XSS can't read it, and SameSite=Lax already
covers CSRF. The narrow Path provides no additional security and
creates a discrepancy with the rest of the cookie set. Cosmetic, but
worth aligning.

**Fix:** Widen to `Path=/auth` for consistency with session/CSRF
cookies.

### M-5. CBL URL refresh path inherits H-1 with no per-user import quota

Same code path as H-1, but the refresh scheduler can hit it on a
recurring cadence the user controls. Once H-1 is fixed the SSRF is
closed, but a per-user import rate limit (e.g. 10/hour, or restrict
the `kind = 'url'` form to admins) is a good complement.

---

## Low

### L-1. Argon2 pepper has no rotation path

[crates/server/src/auth/password.rs:26-35](../../crates/server/src/auth/password.rs#L26)
— pepper is loaded once, never versioned. Rotating it after a
suspected leak invalidates every stored password. Document the
runbook (force a password reset cycle) or add a `pepper_version`
column + opportunistic rehash on next login.

### L-2. Email-token verifier accepts up to 30 days in the future

[crates/server/src/email/token.rs](../../crates/server/src/email/token.rs)
— clock-skew window is much larger than needed. No exploit because
tokens are single-use, but tighten to ±300s for hygiene.

### L-3. CBL multipart upload accepts any `Content-Type`

[crates/server/src/api/cbl_lists.rs:784-793](../../crates/server/src/api/cbl_lists.rs#L784)
— parser rejects non-XML, but failing earlier on Content-Type avoids
spending parse cycles on garbage.

### L-4. `LIMIT` clause uses `format!` interpolation — *not* injection but flag for tidiness

[crates/server/src/api/people.rs:147](../../crates/server/src/api/people.rs#L147)
— `format!(" LIMIT {limit}")` is safe (integer-typed, clamped on line
89), but switching to SeaORM's `.limit()` builder is a one-line
cleanup that removes a flag for future auditors.

---

## What's already done well

Confirmed across the audit:

- **Password hashing.** Argon2id m=64 MiB, t=3, p=1, server-side
  pepper, constant-time dummy verify on missing user.
- **Sessions.** `__Host-` / `__Secure-` cookie prefixes, HttpOnly,
  Secure, SameSite=Lax. Revocation via `token_version` bump. Logout
  clears cookies with Secure flag.
- **CSRF.** Double-submit token, constant-time compare, exempts
  limited to OIDC callback and Bearer-auth flows.
- **OIDC.** PKCE + state + nonce, ID-token signature +
  aud/iss/exp/nbf, 5 min discovery cache, RP-logout, email-collision
  handling.
- **AuthZ.** All 30+ admin handlers use `RequireAdmin` (verified by
  sweep of `crates/server/src/api/admin_*.rs`). Spot-checked
  user-owned resource handlers (saved views, CBL lists, collections,
  pages, markers) — all verify caller ownership before returning
  data or accepting mutations. Cross-user reads return 404, not 403,
  avoiding existence leaks.
- **Input bounds.** OPDS search caps query at 200 chars; people
  search caps at `MAX_QUERY_LEN`; CBL imports cap at 4 MiB / 5000
  entries; archive limits (entries, total bytes, ratio, nesting) are
  env-tunable and enforced at parse time.
- **Path traversal.** Scanner canonicalizes folders and enforces
  `starts_with(library_root)`. Archive entry names reject `..`,
  absolute paths, and backslashes (zip-slip).
- **XXE.** `quick-xml` rejects `<!DOCTYPE` with `DoctypeRejected` and
  never resolves external entities. Worth adding a regression test
  that asserts the rejection.
- **Secrets at rest.** Master keys wrapped in `Zeroizing<[u8;32]>`;
  DB secret rows sealed with XChaCha20-Poly1305 (random nonce per
  encryption).
- **Token storage.** Web client uses HttpOnly cookies only — no
  `localStorage`/`sessionStorage` for tokens. No XSS-readable token
  path.
- **Boot banner.** Logs version/bind/public_url/auth_mode only. No
  database URL, no Redis URL, no secrets.
- **Audit log.** Append-only at the table level; every mutating admin
  handler emits via `crate::audit::record`.
- **PII hashing.** OIDC subject + email logged as sha256 prefix;
  failed-auth logs hash the identifier.
- **Security headers.** Strict CSP, HSTS, X-Frame-Options DENY,
  Referrer-Policy `no-referrer` on auth pages, CORP/COEP.
- **SQLx query logging disabled** (no bound parameters in logs).

---

## Agent findings rejected as false positives

For traceability — these were flagged by the audit agents but
verified safe:

- **"SQL injection in people.rs:140 / :147"** —
  `format!("${}", params.len())` only constructs PostgreSQL
  placeholder *names* (`$1, $2, ...`); the values themselves go
  through `Value::from()` and `Statement::from_sql_and_values`,
  which parameterizes. `LIMIT {limit}` interpolates an `i64` that's
  already clamped — integer types cannot be injected.
- **"No length validation on OPDS search `q`"** —
  [opds.rs:397](../../crates/server/src/api/opds.rs#L397) caps at
  200 chars.
- **"Dummy hash race in OnceLock"** — `OnceLock::get_or_init` is
  sound under concurrent first-use.
- **"Refresh-token rotation fragile under partial failure"** — by
  design; not a security bug.

---

## Recommended remediation order

1. **Now (release blocker for multi-user deploys):** H-1 SSRF guard
   on `create_from_url`.
2. **This week:** H-2 custom `Debug` for `Config`; H-3 IP-keyed
   failed-auth counter + OPDS bucket split.
3. **Next sprint:** M-1 explicit `CorsLayer`, M-2 strip issuer from
   public auth-config, M-3 permission check on key files, M-4 align
   state cookie path, M-5 per-user import quota.
4. **Backlog hygiene:** L-1 through L-4.
