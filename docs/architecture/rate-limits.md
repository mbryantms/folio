# Rate Limits

Phase 0 deliverable. Source: [comic-reader-spec.md §17.7](../../comic-reader-spec.md).

Implementation: `tower_governor` token-bucket. Backend = Redis when `COMIC_REDIS_URL` is set (the default in prod), in-memory otherwise (dev only).

## Identity resolution

- **Per-user** = JWT `sub` (cookie or Bearer) **or** OPDS app-password ID.
- **Per-IP** = first untrusted hop in `X-Forwarded-For`, computed by walking the chain right-to-left and stopping at the first IP **not** in `COMIC_TRUSTED_PROXIES`. If all hops are trusted (impossible in practice), the connection's peer address is used.

Without `COMIC_TRUSTED_PROXIES` set, every request appears to come from the reverse proxy — the per-IP bucket fails open. **Required configuration for any prod deploy.**

## Buckets

| Bucket | Per-IP | Per-user | Burst |
|---|---|---|---|
| `GET /issues/.../pages/.*` | 60 req/s | 300 req/s | 120 / 600 |
| `GET /issues/.../pages/.*/thumb` | 200 req/s | 1000 req/s | 400 / 2000 |
| `GET /search` | 5 req/s | 20 req/s | 10 / 40 |
| `GET /search/autocomplete` | 30 req/s | 60 req/s | 60 / 120 |
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
| Marker writes (`POST /me/markers`, `PATCH`/`DELETE /me/markers/{id}`, `POST /me/markers/bulk-delete`), one shared bucket | 10 / s | — | 600 |

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
