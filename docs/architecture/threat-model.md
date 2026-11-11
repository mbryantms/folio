# Threat Model

Phase 0 deliverable per [comic-reader-spec.md §17](../../comic-reader-spec.md). Single-page, structured by trust boundary and STRIDE per component. Updated as new components ship.

## Assets

1. User reading content (private but not high-value).
2. User progress, ratings, reviews, collections, reading lists.
3. User credentials (local password hashes; OIDC tokens; OPDS app passwords).
4. Server-side secrets (URL-signing key, app-password pepper, JWT signing key when local mode is the only auth).
5. Library files on disk (user owns; read-only to the app unless a library turns on `allow_archive_writeback`, see [archive-writes.md](../dev/archive-writes.md)).

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
                                │  Library FS      │  (RO unless a library has writeback on)
                                │  /data FS        │  (RW; secrets here)
                                └──────────────────┘

External auth issuer (Authentik / Keycloak / Dex) — trusted for identity claims only.
External OPDS readers (Chunky, Panels, KOReader) — credential = app password (Basic or Bearer), reader routes only.
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
| **D**oS | The limits in [rate-limits.md](rate-limits.md) (attached per route, not to every route); CSP report endpoint capped (64 KiB body, selected fields logged); request bodies capped (2 MiB default), and list-shaped bodies capped by count with a field-level 422: sidebar layout 1,000 entries, reader key bindings 128 (64 characters each), page reorder 100 ids, saved-view pin reorder 1,000, view-group layout 50 groups and 10,000 ids per list, log-widget reorder 500, collection-entry reorder 50,000 (collections have no size limit; that sits just under what 2 MiB already allows); JSON `/api` routes time out after 60 s. Queries compiled from a saved-view filter run under `SET LOCAL statement_timeout = 5s` in a read-only transaction (`views::timebox`; a cancelled one answers 422 `filter_too_slow`, OPDS serves the feed empty), and the filter itself is capped at 50 conditions and 200 values per list on writes and previews, with a per-user `saved_view_preview` bucket on the previews. Every other query still runs with no `statement_timeout` (not implemented); statements slower than 500 ms are logged at WARN. |
| **E**levation | Role check on admin endpoints; admin bootstrap warning (§12.8); never honor issuer claims for role assignment. |

Verified against the code at `ed5d9019` on 2026-11-10 (DoS row).

### Library scanner

| Threat | Mitigation |
|---|---|
| Archive bombs / quines | Every archive is parsed in-process (CBZ `zip`, CBT `tar`, CBR through the `unrar` crate's bindings to the UnRAR library, CB7 `sevenz-rust2`) under `ArchiveLimits` (`crates/archive/src/lib.rs`): at most 50,000 entries, 512 MiB per entry, 8 GiB total uncompressed. CBZ drops an entry whose compression ratio exceeds 200:1; CB7 refuses an archive over 200:1 overall and caps decoder memory at 256 MiB. Archives nested inside an archive are not opened. No process isolation, wall-clock or memory limit applies to archive work (the `subprocess_*` limits are unused). Readers read only the file handle the caller opened, never the path; a multi-part RAR is refused (the UnRAR library would open the other parts by name). Where the RAR reader cannot reopen its handle through `/proc/self/fd` it uses the stored path and checks before and after each pass that the path still names the opened file. |
| Zip-slip | Entries are read into memory and never extracted to disk. Entry-name validation (`archive::entry_name`) rejects absolute paths, `.` and `..` components, empty components, backslashes and NUL; other control characters are allowed. CBT keeps regular-file entries only; CBZ and CBR skip directory entries. |
| Subprocess RCE | Not applicable: the server runs no subprocess. The `unrar` binary in the container image is not invoked; RAR is read in-process. |
| Image decoder CVEs | The server decodes JPEG, PNG, WebP and GIF in-process with the `image` crate (thumbnails, page variants, OCR, cover hashes) through `util::image_decode::decode_limited`, which refuses an image wider or taller than 20,000 px. AVIF and JPEG XL pages are content-sniffed and served as stored; the server does not decode them. No decoder sandbox exists (not implemented). |
| Path traversal in scanner | Symbolic links below a library root are never followed, wherever they point: the scanner classifies entries without following links and walks with `follow_links(false)` / `follow_root_links(false)`, so a linked publisher folder, series folder or archive is not indexed; each is reported as a `SymlinkSkipped` health finding. A single-series rescan refuses a folder that is, or sits under, a link. The configured root path itself may be a link (resolved once, as `validate_library` canonicalizes it). Serving routes (OPDS download, page bytes, OPDS page stream, thumbnails, OCR) open the stored path through `library::no_follow`: one name at a time from the root with `O_NOFOLLOW`, regular files only, otherwise not-found. An issue rescan checks the stored path the same way before ingesting. Every reader of a library file (page bytes and stream, OPDS download, thumbnails, OCR, page hashes, scanner ingest and hashing, thumbnail generation, deep validation, cover hashing, the archive editor) opens it once through that walk and reads only through the handle; the archive crate never reopens a path (CBR, whose library opens by name, is given `/proc/self/fd/<n>` for the handle). The page-reader cache reuses a reader only for the same version of the same file (device, inode, size, modification and status-change time) the request just opened. Every writer (sidecar rewrite, page editor, restore, CBR/CB7 conversion, backup prune, staging cleanup) opens the archive's folder through the walk and creates, links, renames and deletes relative to that folder handle; staging files are created with `O_EXCL` and `O_NOFOLLOW` under an unpredictable name; an archive, backup or destination that is a link is refused, and a writeback job that meets one ends as a refusal. Hard links cannot be told apart from ordinary files and are deliberately not refused (download tools commonly hard-link). The OPDS download refuses soft-removed issues. Only people you trust should be able to write to a library folder: whatever the scanner indexes there is served to that library's readers. |
| Library-mount writability | No startup check (not implemented). Folio writes under a library root only when that library has `allow_archive_writeback` on ([archive-writes.md](../dev/archive-writes.md)); mount the library read-only when no library needs writeback. Those writes never follow a link below the root: each works relative to the archive's folder opened through the no-follow walk (see the scanner row and archive-writes.md). A conversion never replaces a file at its destination name (no-replace rename), and no write sequence leaves the issue row naming a file that is not there. |

Verified against the code at `74a1c647` on 2026-11-10.

### WebSocket (`/ws/scan-events`)

Automerge sync was dropped (spec §9 decision note); progress syncs over plain HTTP. The one WebSocket endpoint is the admin-only scan-progress stream.

| Threat | Mitigation |
|---|---|
| Spoofed WS upgrade | One-shot ticket from `POST /auth/ws-ticket` (UUID, 5 min TTL, consumed with Redis `GETDEL`), or the cookie session; the user row is re-read on upgrade and must be an active admin. Ticket minting is limited to 30/s/IP (burst 60). |
| DoS via message floods | Client messages capped at 64 KiB, frames at 16 KiB. |
| Cross-user leak | Admin-only stream; non-admins get 403. |

Verified against the code at `1c51a486` on 2026-11-10.

### OPDS (external readers)

Accepted auth carriers on `/opds/*`: cookie session (web preview), `Authorization: Bearer <jwt|app_…>`, and `Authorization: Basic <b64(user:app_…)>`. Basic is restricted to `app_…` tokens — a raw JWT carried via Basic is rejected by the extractor (clients log/leak the `Authorization` header in places they shouldn't).

App passwords (`app_…`, Basic or Bearer) authenticate only on the reader-client routes, which `app::reader_surface` marks with `AppPasswordSurface`: OPDS 1.2 and 2.0 (`/opds/*`, including downloads, PSE pages, progress writes and the KOReader sync shim), OPDS progression (`/opds/v1/progression/{id}`), the Komga shim (`/api/v1/books/*`), page bytes (`/issues/{id}/pages/{n}`) and thumbnails and stored covers (`…/pages/{n}/thumb`, `/issues/{id}/covers/{cover_id}`). On every other authenticated route (the `/api` JSON API, admin, account and settings, `/auth/me` and the other signed-in `/auth/*` routes) an app password gets 403 `auth.permission_denied` before any lookup; `/ws/scan-events` refuses it with 401. An app password can therefore read what its user can read, subject to the same library ACL and age-rating cap, and, with `read+progress` scope, write reading progress. It cannot change the account, settings, ratings, reviews, collections or anything admin, and cannot reach the JSON API, even when its user is an admin.

| Threat | Mitigation |
|---|---|
| Brute-force on Basic Auth | Tokens are 256 random bits, stored as argon2id + pepper plus a SHA-256 lookup digest: a presented token is found by one indexed lookup and only that row is argon2-checked, so a wrong token costs no hashing once every row has its digest (rows from before the digest are tried at most 20 per request until each is used once). App-password misses count on their own per-IP lockout, separate from sign-in's (10 in 60 s, then 429 for 15 min, checked before any lookup or hashing, a miss charged before the legacy scan hashes; a refused token repeated within 5 min is answered from a refusal cache and not counted again; see [rate-limits.md](rate-limits.md#failed-auth-lockout)). App-password argon2 runs on the blocking pool behind 2 global permits; a request that can't get one within 2 s gets 503. Per-IP buckets `opds_catalog` (30/min) and `opds_stream` (120/min) on `/opds/*`. |
| Stolen app password = full account | Reader routes only (above); two scopes (`read` / `read+progress`); revocable per token from account settings. |
| URL-signature forgery (PSE) | HMAC-SHA256 with server-side key; ACL re-check on each request. |

Verified against the code at `5a1d2f64` on 2026-11-10.

### Email (when SMTP configured)

| Threat | Mitigation |
|---|---|
| Account-existence oracle on reset | `request-password-reset` always returns 204 (the form path always redirects to the same confirmation). |
| Email account compromise → account takeover | Not mitigated: TOTP is not implemented, so control of the mailbox is enough to reset a local password. The reset link expires after 1 h, and a reset bumps `token_version` and revokes every session. Protect the mailbox the account uses. |
| Email-link replay | Reset link single-use via `password_reset_uses` row. |
| Bulk-sending heuristics tripping prod | No install-wide send cap (not implemented). Per-IP limits on the routes that send mail: `request-password-reset` 5/hour (burst 5), `resend-verification` 5/hour (burst 5), `register` 5/min (burst 10). |

Verified against the code at `1c51a486` on 2026-11-10.

### Postgres

| Threat | Mitigation |
|---|---|
| SQL injection in raw-SQL allowlist | Strict policy (§E6 of review): no string interpolation of user values; sort columns from server allowlist. |
| Search-DoS via complex `tsquery` | `websearch_to_tsquery` only; query length cap 200 bytes; per-user `search` bucket (10/s, burst 60) on the search routes. No `statement_timeout` on the search queries (not implemented); only the saved-view filter queries are time-boxed (5 s, see the DoS row). |
| Migration race on multi-replica | `COMIC_AUTO_MIGRATE=false` documented in `docs/install/scaling.md`. |

Verified against the code at `ed5d9019` on 2026-11-10 (search row).

### `/data` mount

| Threat | Mitigation |
|---|---|
| Unauthorized access to thumbnails | Served by Rust binary (auth + ACL checked before bytes); content-addressed paths. |
| Secret leak via backup | `/data/secrets/` flagged as sensitive in §12.5 backup docs. |

## Out-of-scope (assumed)

- Reverse proxy (Caddy/Traefik/nginx) is correctly configured. Reference Caddyfile in `docs/install/caddy.md`.
- Postgres is reachable only by the app; not exposed to the public internet.
- Library files are read-only to the app's user, unless a library has `allow_archive_writeback` on.
- Only trusted people can write to a library folder (see the scanner table).
- The OIDC issuer is trustworthy. If it's compromised, every account that uses it is.

## Review cadence

Re-evaluate whenever:
- A new component crosses a trust boundary (e.g., enrichment HTTP client → §17.11).
- A new authn/authz path is added.
- A new file format is parsed.
- A new external-facing endpoint ships.

Keep the doc terse. If it gets long, the threat model is wrong.
