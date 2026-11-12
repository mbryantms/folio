## Runtime configuration

Folio splits its config into two layers:

- **Infrastructure / boot-required** stays in `.env` (the `COMIC_*`
  prefix). These values are read before the DB connection is established
  and cannot be overridden at runtime.
- **Policy / operator-tunable** lives in the `app_setting` table and is
  editable from the admin UI under `/admin/auth`, `/admin/email`, and
  `/admin/server`. Changes take effect on the next request (or, for the
  M5 operational-tuning slice, on the next restart).

Plan: [`~/.claude/plans/runtime-config-admin-1.0.md`](../../../.claude/plans/runtime-config-admin-1.0.md).

### What stays in `.env`

| Key | Why it can't move |
|---|---|
| `COMIC_DATABASE_URL` | Needed before any DB row can be read. |
| `COMIC_REDIS_URL` | Needed before apalis can spawn. |
| `COMIC_LIBRARY_PATH`, `COMIC_DATA_PATH` | Filesystem mounts. |
| `COMIC_BIND_ADDR` | The HTTP listener binds before the router is built. |
| `COMIC_PUBLIC_URL` | Used to build the OIDC redirect URI; OIDC clients depend on a stable redirect. |
| `COMIC_TRUSTED_PROXIES` | Feeds the XFF walker installed at router-build time. |
| `COMIC_AUTO_MIGRATE` | Decides whether to run migrations before serving. |
| `COMIC_LOAD_DOTENV` | Boot-time flag that gates `.env` file loading; meaningless after boot completes. |
| `COMIC_GITHUB_TOKEN` | Read by the CBL-catalog refresher on each fetch; not a moveable policy setting (per-deploy credential, not user-visible config). |
| `COMIC_METRICS_TOKEN` | Bearer token gating `GET /metrics` in production/release builds; a deploy-time scrape credential baked into Prometheus config, not user-visible policy. |
| `COMIC_METRICS_OPEN` | Explicit production opt-out for unauthenticated `/metrics`; leave unset unless a reverse proxy or network ACL protects the route. |
| `COMIC_OCR_LEGACY_ENGINES` | **Temporary (one release).** `true` runs the previous text-reading engines (`comic-text-detector` + Tesseract) instead of the current ones, so a deploy can go back without a code revert. Which engines a binary runs is a deploy decision, read once at start; it is removed together with the previous engines. See [ocr.md](ocr.md#engine-switch-comic_ocr_legacy_engines). |

Server-generated cryptographic keys are neither env nor DB: they are files
under `${COMIC_DATA_PATH}/secrets/` (`pepper`, `jwt-ed25519.key`,
`email-token.key`, `url-signing.key`, `settings-encryption.key`, and the
optional `pepper.previous` used only during an argon2 pepper rotation —
WP-6.3 / security audit L-1). They stay out of `app_setting` because the
settings key itself lives there and a DB-only leak must not yield the
pepper; they stay out of env so they never show up in `docker inspect` or
process listings. Rotation runbook:
[`docs/install/secrets-backup.md`](../install/secrets-backup.md#rotating-the-pepper).

Compose-only keys (`REPO_OWNER`, `TAG`, `POSTGRES_PASSWORD`,
`COMIC_LIBRARY_HOST_PATH`, `COMIC_APP_BIND`, `COMIC_WEB_BIND`) stay in
`.env` indefinitely — they describe the deployment topology, not server
config.

### What moved to the admin UI

| Key (`app_setting` table) | Admin page | Effect |
|---|---|---|
| `smtp.{host,port,tls,username,password,from}` | `/admin/email` | Live — the `EmailSender` is rebuilt on save. |
| `auth.mode` | `/admin/auth` → Mode | Live. Optional: with no row and no `COMIC_AUTH_MODE` the effective mode is `local` when no OIDC issuer / client id / client secret is set and `both` when all three are; only some set is an error naming the missing values ([`config::resolve_auth_mode`](../../crates/server/src/config.rs), run by `Config::resolve_auth` after env load, after the DB overlay and in the PATCH dry-run). An explicit `oidc` / `both` without all three is rejected. The admin page shows the effective mode and whether it was saved or defaulted. |
| `auth.local.registration_open` | `/admin/auth` → Local | Live. |
| `auth.oidc.{issuer,client_id,client_secret,trust_unverified_email,link_local_by_verified_email,auto_redirect,grant_all_libraries}` | `/admin/auth` → OIDC | Live; OIDC discovery cache is evicted on save. |
| `auth.jwt.{access_ttl,refresh_ttl}` | `/admin/auth` → Tokens | Live for newly-minted tokens; existing tokens keep their original `exp`. Defaults `1h` / `30d` (the access default was `24h` before; installs that saved a value keep it). |
| `auth.session.max_age` | `/admin/auth` → Tokens | Live — checked at every `/auth/refresh`: a refresh session cannot be rotated past `created_at` + this, whatever its sliding expiry says (401 `auth.invalid`, row revoked). Default `90d`, floor `1d`, ceiling `3650d` (the access and refresh TTLs share that ceiling; larger values answer 422, and a stored value above it is clamped), must be ≥ the access TTL. Also clamps the refresh and CSRF cookie Max-Age, and hides older sessions from `/me/sessions`. No bootstrap row: a missing row means the env baseline. `COMIC_SESSION_MAX_AGE` is read through the generic `COMIC_` prefix (figment `Env::prefixed`) and is that baseline, validated at startup; a saved row wins over it. It works, but prefer the admin setting like the other policy keys; the env value is never copied into the table. |
| `auth.rate_limit_enabled` | `/admin/server` → Hardening | Live, but only affects the failed-auth Redis lockout and the CBL import quota's hourly window. The per-route `tower_governor` buckets are sized at boot and stay on when it is `false`. |
| `observability.log_level` | `/admin/server` → Diagnostics | Live via `tracing_subscriber::reload::Handle`. |
| `library.hard_purge_multiplier` | `/admin/server` → Removed-file purge | Live — the 04:15 hard-purge sweep reads it each run. Confirmed-removed rows are deleted after `soft_delete_days × this` days; `0` disables; range `[0, 100]`, default `2` (WP-3.5, see [library-scanner.md](library-scanner.md#removal-lifecycle)). Env bootstrap: `COMIC_LIBRARY_HARD_PURGE_MULTIPLIER`. |
| `audit_log.retention_days` | `/admin/server` → Audit log retention | Live — the 03:45 UTC sweep ([`jobs::audit_log_retention`](../../crates/server/src/jobs/audit_log_retention.rs)) reads it each run. `0` (default) keeps every `audit_log` row forever. When set (floor `90`, ceiling `36500`), rows older than this many days are first exported as newline-delimited JSON to `${COMIC_DATA_PATH}/audit-log-exports/audit-log-<timestamp>.ndjson` (fsync + atomic rename) and only then deleted in batches of 1000; a failed export deletes nothing. The job is the one sanctioned deleter of `audit_log` — handlers stay append-only ([observability.md](observability.md#server-stream)). Env bootstrap: `COMIC_AUDIT_LOG_RETENTION_DAYS` (audit 2026-11 E1). |
| `cache.zip_lru_capacity` | `/admin/server` → Caching | **Applies on next restart** (LRU sized at boot). |
| `cache.thumbs_budget_mb` | `/admin/server` → Thumbnail storage | Live — read by every budget sweep (hourly, after the daily orphan sweep, and throttled after thumbnail writes). `0` / unset = unlimited (default). When set (≥ 64 MiB), generated thumbnails are evicted least-recently-used first — page strips before covers — down to 90% of the budget; they re-render on demand. Downloaded provider covers under `thumbs/issues/` are counted but never evicted. No env var (new in WP-3.8). Size is exported as `folio_thumbs_bytes` ([metrics.md](metrics.md)). |
| `workers.{scan_count,scan_folder_concurrency,post_scan_count,scan_batch_size,scan_hash_buffer_kb,archive_work_parallel,thumb_inline_parallel}` | `/admin/server` → Workers | **Applies on next restart** (apalis pool size fixed at startup). `scan_folder_concurrency` (post-audit T1.2, default 2) is the per-scan folder fan-out; `scan_count × scan_folder_concurrency` bounds concurrent scan transactions. |
| `scanner.{watch_debounce_secs,watch_poll_interval_secs}` | `/admin/server` → File watcher | Live — the save nudges the watcher supervisor, which restarts any watcher whose timing changed (WP-3.1). Env bootstrap: `COMIC_WATCH_DEBOUNCE_SECS` (default 30, range 1–3600), `COMIC_WATCH_POLL_INTERVAL_SECS` (default 300, range 30–86400). `COMIC_WATCH_FORCE_POLL` stays env-only (it describes the mount, i.e. infrastructure). |
| `metadata.comicvine.{api_key,enabled}`, `metadata.metron.{api_token,username,password,enabled}`, `metadata.gcd.{username,password,enabled}` | `/admin/metadata` → Providers | Live — provider clients are built per job/request from the current `Config`. `api_token` (secret, `Bearer`) is preferred over the username/password pair when set (WP-2.9). GCD is HTTP Basic; `password` is secret (WP-6.1). Env bootstrap: `COMIC_COMICVINE_API_KEY`, `COMIC_METRON_API_TOKEN`, `COMIC_METRON_USERNAME`, `COMIC_METRON_PASSWORD`, `COMIC_GCD_USERNAME`, `COMIC_GCD_PASSWORD`. |

### Precedence (D1)

When both env and DB set the same key, **the DB value wins**. The env
value is used as a boot-time default until a DB row exists for that key.
On every PATCH the server logs a `WARN` if env and DB disagree, so
operators can spot stale `.env` lines being shadowed.

### Encryption at rest

Secret rows (`smtp.password`, `auth.oidc.client_secret`) are sealed with
XChaCha20-Poly1305 using the key in
`${COMIC_DATA_PATH}/secrets/settings-encryption.key`. This key is
auto-generated on first boot alongside the other entries managed by
[`crate::secrets`](../../crates/server/src/secrets.rs). Backup the
`secrets/` directory.

The API never echoes the plaintext of a secret row — `GET /admin/settings`
returns `"<set>"` and the form's "leave blank to keep" placeholder is
the only way to indicate "no change."

### Dry-run validation

Every `PATCH /admin/settings` runs the proposed Config through
`Config::resolve_auth` + `Config::validate` *before* writing to DB.
Invalid combinations (`auth.mode=oidc` without OIDC creds; part of the
OIDC block saved while no mode is chosen; `refresh_ttl < access_ttl`;
`session_max_age` under `1d` or under the access TTL; worker counts out
of `[1, 64]`; bogus `EnvFilter` directive) return
`422 settings.invalid_combination` and persist nothing.

### Bootstrap (env → DB on first boot)

For each slice we own (`smtp.*`, `auth.*`, `auth.jwt.*` + diagnostics,
operational tuning), the server seeds the DB on first boot from the
env-loaded `Config` when the slice's sentinel row is absent. Subsequent
boots see the row and skip. This lets an existing
`compose.prod.yml`-driven deployment upgrade in place without losing
config — the admin UI sees the env-derived values and the operator can
edit from there.

The `auth.*` slice is the one exception to "copy the effective value":
`auth.mode` is seeded only when `COMIC_AUTH_MODE` was set, so a defaulted
mode stays unchosen and keeps following `resolve_auth_mode`. Its sentinel
is `auth.mode` or `auth.local.registration_open`.

### Deferred (post-1.0)

- **Archive limits** (`COMIC_ARCHIVE_MAX_*`): spec'd in §4.1.1 but all
  three call sites in the server crate still use
  `archive::ArchiveLimits::default()`. Adding admin UI fields without
  threading them through to the enforcement points would be misleading,
  so we left them out of M5. The env vars in `.env.example` are
  commented out and labelled as no-ops.
- **Live worker-pool restart**: the apalis monitor is spawned once at
  startup. The admin UI sets `workers.*` rows but the running pool
  doesn't pick them up until the next process restart. Live restart
  requires a `Scheduler::restart_pools` implementation that gracefully
  drains in-flight jobs; tracked as future work.
- **Per-route `tower_governor` toggle**: the `auth.rate_limit_enabled`
  kill switch covers the failed-auth Redis lockout (the operationally
  painful case) and the CBL import quota's hourly window, but the per-route buckets stay installed regardless.
  Disabling them at runtime needs a custom tower `Service` wrapping each
  `GovernorLayer`. Tracked as future work.
- ~~**OTLP exporter** (`COMIC_OTLP_ENDPOINT`)~~: **considered, not chosen
  for v1** (2026-05-15). The env var stays read by `Config` but
  observability.rs logs a clear "intentionally not shipped" hint when it's
  set. The opentelemetry crate matrix has a volatile compat story; no real
  user demand has surfaced; Prometheus `/metrics` already covers
  operator-monitoring. Re-evaluate when a hosted-Folio deployment ships or
  a user reports a need Prometheus can't cover. See
  [docs/dev/incompleteness-audit.md §D-9](incompleteness-audit.md#d-9-otlp-exporter-wiring--resolved-2026-05-15-considered-not-chosen).
- **Provenance badges** (`GET /admin/settings` returning source: env vs
  DB vs default): plumbing exists in the `Provenance` enum but isn't
  surfaced in responses or the UI yet.

### Add a new setting

1. Add a `SettingDef` to the
   [`REGISTRY`](../../crates/server/src/settings/registry.rs) const.
   Mark `is_secret: true` if the value should be sealed before storage
   and redacted in responses.
2. Add a match arm in
   [`apply_overlay_row`](../../crates/server/src/config.rs) that binds
   the row's value onto the corresponding `Config` field.
3. If the new key triggers a live side-effect (rebuild email,
   evict cache, reload tracing filter), add an `affects_*` predicate to
   the registry and a step in
   [`admin_settings::update`](../../crates/server/src/api/admin_settings.rs)
   that fires after `replace_cfg`.
4. Add range / cross-field validation to `Config::validate`. Dry-run
   will surface it as a 422 before any write. A setting whose effective
   value is *derived* when unset (as `auth.mode` is) keeps the operator's
   choice in its own `Option` field and resolves it in one function called
   from `load`, `overlay_db` and the dry-run, never per reader.
5. Wire a bootstrap seeder in
   [`settings/bootstrap.rs`](../../crates/server/src/settings/bootstrap.rs)
   so a fresh upgrade from env populates the row on first boot.
6. Add a deprecation `WARN` for the matching env var in `app.rs::serve`
   (with `target = "comic.deprecation"`, `since = "..."`) and annotate
   `.env.example`.
7. Add a card under the appropriate `/admin/*` page.
