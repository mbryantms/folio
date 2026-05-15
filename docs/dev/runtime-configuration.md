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

Compose-only keys (`REPO_OWNER`, `TAG`, `POSTGRES_PASSWORD`,
`COMIC_LIBRARY_HOST_PATH`, `COMIC_APP_BIND`, `COMIC_WEB_BIND`) stay in
`.env` indefinitely — they describe the deployment topology, not server
config.

### What moved to the admin UI

| Key (`app_setting` table) | Admin page | Effect |
|---|---|---|
| `smtp.{host,port,tls,username,password,from}` | `/admin/email` | Live — the `EmailSender` is rebuilt on save. |
| `auth.mode` | `/admin/auth` → Mode | Live. |
| `auth.local.registration_open` | `/admin/auth` → Local | Live. |
| `auth.oidc.{issuer,client_id,client_secret,trust_unverified_email}` | `/admin/auth` → OIDC | Live; OIDC discovery cache is evicted on save. |
| `auth.jwt.{access_ttl,refresh_ttl}` | `/admin/auth` → Tokens | Live for newly-minted tokens; existing tokens keep their original `exp`. |
| `auth.rate_limit_enabled` | `/admin/server` → Hardening | Live, but only affects the failed-auth Redis lockout. The per-route `tower_governor` buckets are sized at boot. |
| `observability.log_level` | `/admin/server` → Diagnostics | Live via `tracing_subscriber::reload::Handle`. |
| `cache.zip_lru_capacity` | `/admin/server` → Caching | **Applies on next restart** (LRU sized at boot). |
| `workers.{scan_count,post_scan_count,scan_batch_size,scan_hash_buffer_kb,archive_work_parallel,thumb_inline_parallel}` | `/admin/server` → Workers | **Applies on next restart** (apalis pool size fixed at startup). |

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
`Config::validate` *before* writing to DB. Invalid combinations
(`auth.mode=oidc` without OIDC creds; `refresh_ttl < access_ttl`;
worker counts out of `[1, 64]`; bogus `EnvFilter` directive) return
`400 settings.invalid_combination` and persist nothing.

### Bootstrap (env → DB on first boot)

For each slice we own (`smtp.*`, `auth.*`, `auth.jwt.*` + diagnostics,
operational tuning), the server seeds the DB on first boot from the
env-loaded `Config` when the slice's sentinel row is absent. Subsequent
boots see the row and skip. This lets an existing
`compose.prod.yml`-driven deployment upgrade in place without losing
config — the admin UI sees the env-derived values and the operator can
edit from there.

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
  painful case) but the per-route buckets stay installed regardless.
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
   will surface it as a 400 before any write.
5. Wire a bootstrap seeder in
   [`settings/bootstrap.rs`](../../crates/server/src/settings/bootstrap.rs)
   so a fresh upgrade from env populates the row on first boot.
6. Add a deprecation `WARN` for the matching env var in `app.rs::serve`
   (with `target = "comic.deprecation"`, `since = "..."`) and annotate
   `.env.example`.
7. Add a card under the appropriate `/admin/*` page.
