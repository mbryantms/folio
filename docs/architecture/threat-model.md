# Threat Model

Phase 0 deliverable per [comic-reader-spec.md §17](../../comic-reader-spec.md). Single-page, structured by trust boundary and STRIDE per component. Updated as new components ship.

## Assets

1. User reading content (private but not high-value).
2. User progress, ratings, reviews, collections, reading lists.
3. User credentials (local password hashes; OIDC tokens; OPDS app passwords).
4. Server-side secrets (URL-signing key, app-password pepper, JWT signing key when local mode is the only auth).
5. Library files on disk (read-only to the app; user owns).

## Trust boundaries

```
┌────────────┐                  ┌──────────────────┐
│  Browser   │ ◀─── HTTPS ────▶ │  Reverse proxy   │
└────────────┘                  │  (Caddy/Traefik) │
                                └────────┬─────────┘
                                         │  HTTP (intra-host)
                                         ▼
                                ┌──────────────────┐         ┌─────────────┐
                                │  comic-reader    │ ◀────▶  │  Postgres   │
                                │  Rust binary +   │         └─────────────┘
                                │  Next.js         │         ┌─────────────┐
                                └────────┬─────────┘ ◀────▶  │  Redis      │
                                         │                   └─────────────┘
                                         ▼
                                ┌──────────────────┐
                                │  Library FS      │  (mounted RO)
                                │  /data FS        │  (RW; secrets here)
                                └──────────────────┘

External auth issuer (Authentik / Keycloak / Dex) — trusted for identity claims only.
External OPDS readers (Chunky, Panels, KOReader) — bearer credential = app password.
```

Each line is a trust boundary. Anything inside the dotted box is the same trust domain; everything that crosses a line needs an explicit security check.

## Per-component STRIDE

### API gateway (Axum)

| Threat | Mitigation |
|---|---|
| **S**poofing | OIDC PKCE (§17.1); `aud`/`iss`/`exp`/`nbf` validated; JWKS following with `kid` mismatch refresh; `email_verified` defaults to false (§12.7); `users.token_version` for revocation. |
| **T**ampering | Cookie session is signed; CSRF double-submit on unsafe verbs; CSP + COOP/COEP/CORP. |
| **R**epudiation | `audit_log` (§5.9) for admin/security actions; trace IDs in every log line. |
| **I**nformation disclosure | ACL predicate on every read query (§5.1.1); content-type sniffing on page bytes; signed URLs for OPDS-PSE; `nosniff` everywhere. |
| **D**oS | Rate-limit table (§17.7); CSP report endpoint capped; payloads bounded; `statement_timeout=2s` on search queries. |
| **E**levation | Role check on admin endpoints; admin bootstrap warning (§12.8); never honor issuer claims for role assignment. |

### Library scanner

| Threat | Mitigation |
|---|---|
| Archive bombs / quines | §4.1.1 hard caps (entries, total bytes, ratio, nesting); `prlimit` on subprocesses. |
| Zip-slip | Entry-name validation rejects `..`, absolute paths, NUL/control chars, symlinks, devices. |
| Subprocess RCE (`unrar`, `7z`) | argv-only invocation, `--` separator, no shell. |
| Image decoder CVEs (AVIF, JXL) | Wasm-sandbox preferred for AVIF/JXL; native decoders run with `prlimit`. |
| Path traversal in scanner | `library_root.contains(canonicalize(path))` check; symlinks followed only if target is inside library. |
| Library-mount writability | Startup test; refuse to start unless explicit override. |

### Sync (Automerge / WebSocket)

| Threat | Mitigation |
|---|---|
| Spoofed WS upgrade | One-shot ticket (§9.6); ticket validation rate-limited 30/s/IP. |
| DoS via giant docs / message floods | §9.4 bounds (16 MiB/doc, 1 MiB/msg, 200 msg/min, 8 conns/user). |
| Cross-user document leak | Per-user docs; ACL applied at API layer. |
| Document growth runaway | Compaction triggers per §9.3. |

### OPDS (external readers)

Accepted auth carriers on `/opds/*`: cookie session (web preview), `Authorization: Bearer <jwt|app_…>`, and `Authorization: Basic <b64(user:app_…)>`. Basic is restricted to `app_…` tokens — a raw JWT carried via Basic is rejected by the extractor (clients log/leak the `Authorization` header in places they shouldn't).

| Threat | Mitigation |
|---|---|
| Brute-force on Basic Auth | argon2id + pepper; failed-auth bucket (§17.7); 15 min OPDS-only IP lockout. |
| Stolen app password = full account | Two-scope model (`read` / `read+progress`); cannot mutate reviews/ratings/admin. |
| URL-signature forgery (PSE) | HMAC-SHA256 with server-side key; ACL re-check on each request. |

### Email (when SMTP configured)

| Threat | Mitigation |
|---|---|
| Account-existence oracle on reset | `request-password-reset` always returns 202. |
| Email account compromise → account takeover | Reset requires TOTP if user has it enrolled. |
| Email-link replay | Reset link single-use via `password_reset_uses` row. |
| Bulk-sending heuristics tripping prod | 100 mail/hour install-wide cap (`COMIC_SMTP_MAX_PER_HOUR`). |

### Postgres

| Threat | Mitigation |
|---|---|
| SQL injection in raw-SQL allowlist | Strict policy (§E6 of review): no string interpolation of user values; sort columns from server allowlist. |
| Search-DoS via complex `tsquery` | `websearch_to_tsquery` only; query length cap 200; `statement_timeout=2s`. |
| Migration race on multi-replica | `COMIC_AUTO_MIGRATE=false` documented in `docs/install/scaling.md`. |

### `/data` mount

| Threat | Mitigation |
|---|---|
| Unauthorized access to thumbnails | Served by Rust binary (auth + ACL checked before bytes); content-addressed paths. |
| Secret leak via backup | `/data/secrets/` flagged as sensitive in §12.5 backup docs. |

## Out-of-scope (assumed)

- Reverse proxy (Caddy/Traefik/nginx) is correctly configured. Reference Caddyfile in `docs/install/caddy.md`.
- Postgres is reachable only by the app; not exposed to the public internet.
- Library files are read-only to the app's user.
- The OIDC issuer is trustworthy. If it's compromised, every account that uses it is.

## Review cadence

Re-evaluate whenever:
- A new component crosses a trust boundary (e.g., enrichment HTTP client → §17.11).
- A new authn/authz path is added.
- A new file format is parsed.
- A new external-facing endpoint ships.

Keep the doc terse. If it gets long, the threat model is wrong.
