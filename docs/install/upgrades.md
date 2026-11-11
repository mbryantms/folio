# Upgrading Folio

Folio ships container images via GHCR. The compose stack you got from
[`README.md`](../../README.md#getting-started) pins both images to
the tag you set as `TAG` in `.env`. Upgrade is a two-command operation
for routine releases.

## Picking a tag to track

Each release publishes the following tags to
`ghcr.io/mtbry/folio` and `ghcr.io/mtbry/folio-web`:

| Tag        | Example   | What it means                                |
|------------|-----------|----------------------------------------------|
| `vX.Y.Z`   | `v1.2.3`  | Immutable. Pins exactly this release.        |
| `vX.Y`     | `v1.2`    | Floats to the latest patch in `v1.2.*`.      |
| `vX`       | `v1`      | Floats to the latest minor in `v1.*`.        |
| `latest`   | `latest`  | Latest non-prerelease tag.                   |
| `edge`     | `edge`    | HEAD of `main`. Pre-release; useful for testing. |
| `sha-…`    | `sha-abc1234` | Specific commit. Pins traceability for CI/CD. |

For self-hosters the sweet spot is **`TAG=vX.Y`** — automatic patch
upgrades (security fixes, bug fixes) without ever breaking on a major
version bump. Set it once in `.env`:

```env
TAG=v0.1
```

## Routine upgrade

```bash
cd /opt/folio
docker compose -f compose.prod.yml pull
docker compose -f compose.prod.yml up -d
docker compose -f compose.prod.yml logs -f app
```

- `pull` fetches the latest image for the tag you pinned.
- `up -d` recreates only the changed containers; postgres + redis volumes are untouched.
- The `app` container runs `migration::up()` on every boot
  (`COMIC_AUTO_MIGRATE=true`). Migrations are idempotent — already-applied
  ones are no-ops, new ones run in order.
- Tail `app` logs until you see `comic-reader starting` — that's the cue
  that migrations finished and the listener is bound.

`pull` refreshes the images, not your copy of `compose.prod.yml`. The
current file gives Postgres a 256 MB `/dev/shm` (`shm_size`) so parallel
queries on large libraries don't fail with "could not resize shared
memory segment", gives `app` a 35 s `stop_grace_period` so job workers
can finish their 25 s drain, and rotates every container's logs at
10 MB × 5 files. It also runs every container read-only with dropped
capabilities and puts Postgres and Redis on an internal network (see
*Unreleased — hardened compose template* below). If your copy predates
these, download the new `compose.prod.yml` (re-apply any local edits)
before `up -d`.

## Postgres version requirement

Folio requires **Postgres 17 or newer**. The search migration depends on
17+ behaviour, so the server checks `SHOW server_version_num` right
after it connects and — on an older server — refuses to boot *before*
running any migration, with a log line like:

```
refusing to boot: Postgres 16.9 is too old; Folio requires Postgres 17.0 or newer (server_version_num 160009 < 170000) …
```

There is no override. If you run Folio against your own Postgres (not
the one in `compose.prod.yml`, which ships `postgres:18`), upgrade the
server first. Moving an existing volume across a Postgres major is a
`pg_dump` → new server → `pg_restore` operation — `just backup` /
`just restore` wrap that (see [`backup.md`](./backup.md)); the Postgres
image's entrypoint refuses to start on a data directory from a different
major rather than corrupt it.

## Major version bump

Before pinning `TAG=v(X+1)…`:

1. **Read the release notes** at
   <https://github.com/mtbry/folio/releases>. The release body
   calls out breaking changes, env-var renames, and required operator
   steps.
2. **Take a Postgres dump** (see [`backup.md`](./backup.md)). Major
   releases occasionally introduce migrations that have no `down`
   counterpart (sea-orm doesn't require them) — restoring from dump is
   the rollback path.
3. **Snapshot the `comic_data` volume** if your filesystem supports it
   (zfs, btrfs, lvm). The secrets under `${COMIC_DATA_PATH}/secrets/`
   are stable across upgrades, but capturing them is cheap insurance.
4. **Then** `docker compose pull && up -d`.

## Verifying an upgrade

```bash
# 1. App is running the expected version.
curl -fsS http://127.0.0.1:8080/healthz | jq '.version, .build_sha'

# 2. Both deps are reachable.
curl -fsS http://127.0.0.1:8080/readyz | jq

# 3. Web is serving HTML through the Rust binary's SSR fallback.
curl -fsS http://127.0.0.1:8080/sign-in | grep -o '<title>.*</title>'
```

If any of these fail, see the **Rollback** section below.

## Rollback

Migrations in this repo do not implement `down` (Sea-ORM allows
forward-only migrations). The supported rollback procedure is:

```bash
cd /opt/folio

# 1. Stop the stack.
docker compose -f compose.prod.yml down

# 2. Restore the Postgres dump you took before the upgrade.
gunzip -c /var/backups/folio/postgres-2026-05-12.sql.gz | \
  docker compose -f compose.prod.yml run --rm -T postgres \
    psql -U comic -d comic_reader

# 3. Pin TAG to the old version in .env.
$EDITOR .env

# 4. Bring it back up.
docker compose -f compose.prod.yml up -d
```

If you don't have a dump from before the upgrade, you can still roll
back the **image** (set the old `TAG`, `docker compose up -d`) and
attempt the next boot with `COMIC_AUTO_MIGRATE=false` — the old binary
may run fine against the migrated schema for a while, but no guarantee.

## Running migrations as a separate step

Folio is single-instance (see [`scaling.md`](./scaling.md)), so the
default `COMIC_AUTO_MIGRATE=true` is safe: the one app container
migrates at boot. If you'd rather migrate explicitly — to see the
output, or to gate the rollout in CI — set `COMIC_AUTO_MIGRATE=false`
and run the migrator as a one-shot before starting the new image:

```bash
docker compose -f compose.prod.yml run --rm app /app/migration up
docker compose -f compose.prod.yml up -d --no-deps app
```

## Breaking changes by version

### Unreleased — hardened compose template; optional Redis password

- **Image updates need no action.** The `folio` image no longer contains
  the `unrar` program (it was never run: RAR comics are read inside the
  server through the UnRAR library, as before). Nothing else in the
  images changed, and they keep working with the `compose.prod.yml` you
  already have.
- **Adopting the hardened template is optional and manual** (`pull`
  never touches your compose file). Download the new `compose.prod.yml`,
  re-apply your local edits, and run `docker compose up -d`; every
  container is recreated once, volumes and data are untouched. What the
  new file adds, if you prefer to merge it into your own copy:
  - the `x-hardening` block (`read_only: true`, `cap_drop: [ALL]`,
    `security_opt: ["no-new-privileges:true"]`) merged into every
    service with `<<: *hardening`;
  - `cap_add` per service: `app-init` `CHOWN`, `DAC_READ_SEARCH`,
    `FOWNER`; `postgres` `CHOWN`, `DAC_READ_SEARCH`, `FOWNER`, `SETUID`,
    `SETGID`; `redis` `SETUID`, `SETGID`; `app` and `web` none;
  - `tmpfs`: `/tmp` on `app`, `/var/run/postgresql` on `postgres`;
  - `network_mode: none` on `app-init`; `networks: [default, backend]`
    on `app`, `[default]` on `web`, `[backend]` on `postgres` and
    `redis`, and the top-level `networks:` block declaring `backend`
    as `internal: true`;
  - on `redis`: the `environment` (`SKIP_FIX_PERMS`, `REDIS_PASSWORD`),
    the new `command` and `healthcheck`; on `app`: the new
    `COMIC_REDIS_URL` line.

  The `default` network keeps its name, so a reverse proxy attached to
  `<project>_default` still reaches `app:8080`. Postgres and Redis are
  no longer on it: anything else of yours that connected to them over
  that network (a backup container, an admin tool) must join `backend`
  instead. `docker compose exec` (and `just backup` / `just restore`)
  work as before.
- **Optional Redis password.** Add `REDIS_PASSWORD=<value>` to `.env`
  (letters, digits and `- . _ ~` only, e.g. `openssl rand -hex 32`; any
  other character makes Redis refuse to start with a message saying
  so), then run `docker compose -f compose.prod.yml up -d`. Compose
  recreates Redis and the app together, Redis with `requirepass` and the
  app with the password in `COMIC_REDIS_URL`; queued jobs survive (they
  are in the Redis volume). Do not restart only one of the two: an app
  without the password cannot reach a Redis that requires one. To turn
  it off again, remove the line and run `up -d` the same way. Leaving it
  unset keeps today's behaviour. Needs the new template: an older file
  ignores the variable.
- **If you customised your compose file**, check before adopting:
  - *Extra mounts:* only the image's own filesystem is read-only. Bind
    mounts and volumes stay writable as declared, so a writeback library
    still needs its mount without `:ro`. Anything that writes elsewhere
    in a container (a tool you `docker exec` into it, a sidecar sharing
    its filesystem) fails with `Read-only file system`; give it a
    volume or `tmpfs`.
  - *Custom `user:`* on `postgres` or `redis`: their entrypoints then
    skip the root steps and need none of the added capabilities; the
    data volume must already belong to that user.
  - *SELinux / AppArmor:* the template sets no labels or profiles, so
    Docker's defaults apply. The new settings only take permissions
    away, but they were tested on a host with neither enabled: if a
    container fails to start under your policy, check the audit log for
    a denial. On SELinux hosts a bind mount still needs its usual `:z` /
    `:Z` suffix.

### Unreleased — 1 h access tokens by default; sessions end after 90 days

- **The built-in access-token lifetime is now 1 hour** (was 24 hours).
  An existing install does not change: its first boot saved the access
  TTL in the database (`auth.jwt.access_ttl`, normally `24h`), and the
  saved value wins. `/admin/auth` → *Tokens* shows what is in effect; set
  *Access TTL* to `1h` there to adopt the new default. A shorter access
  token limits how long a copied session cookie keeps working after
  sign-out; the web app renews it silently, including on a full page load
  after the access cookie expired. A fresh install gets `1h` (and
  `.env.example` no longer sets `24h`; if your `.env` still carries
  `COMIC_JWT_ACCESS_TTL=24h` it only matters on a fresh database).
- **Sessions have a maximum age, 90 days by default** (new setting
  `auth.session.max_age`, `/admin/auth` → *Tokens* → *Maximum session
  age*, at least `1d`). A sign-in can be refreshed until 90 days after
  it was made, however often it is used; then the user signs in again.
  It applies to existing sessions at once: anyone who last signed in
  more than 90 days before the upgrade is asked to sign in again at the
  next token refresh. Raise the setting first if that is unwelcome.
- **The CSRF cookie lives as long as the refresh cookie** (it used to
  expire with the access cookie, which made the silent refresh fail once
  the access cookie had expired).

### Unreleased — archives refused by a safety limit get their own finding

- A comic the archive reader refuses on a protective limit (entry count,
  entry or total size, compression ratio, 7z decoder memory) or on an
  unsafe or non-portable entry name is now reported as **Archive refused
  by a safety limit**, naming the check, instead of as a malformed
  archive. That includes RAR and 7z files refused while a library
  converts them to CBZ, which were reported as an unsupported format.
  Files already flagged keep their old finding until they are read
  again: an ordinary scan skips a file that has not changed, so run a
  **Content verify** scan (the library's *Scan* menu, or *Scan all* with
  *Content-verify scan* ticked) to re-word them. Migration
  `m20271111_000002_health_issue_scan_idx` adds an index used to count
  each scan run's findings.

### Unreleased — multi-part RAR comics are no longer read

- Comics stored as a multi-part RAR set (`.part1.rar`, `.r00`, …) are
  no longer read. In a library with CBR conversion on, the scan reports
  each as a malformed archive with that reason; with conversion off a
  `.cbr` gets the unsupported-format finding, as before. Repack the
  parts as a single archive (CBZ, or a one-part RAR).

### Unreleased — app passwords are looked up by digest

- **App passwords gain a lookup digest** (migration
  `m20271107_000001_app_password_lookup_digest`). Existing app
  passwords keep working. Each gains its digest the next time a reader
  app presents it successfully; until then they are matched most
  recently used first, 20 per request. On a server with more than 20
  old app passwords, one that has not been used recently may need to be
  re-created. Revoke app passwords nobody uses; the gauge
  `folio_auth_app_password_legacy_remaining` shows how many old ones
  remain.
- **Reader apps presenting a token the server no longer accepts are now
  locked out** per client address after repeated attempts (10 in a
  minute, for 15 minutes). This is the app-password lockout only: web
  sign-in from that address is unaffected. Behind a reverse proxy, set
  `COMIC_TRUSTED_PROXIES` so the address is the client's and not the
  proxy's.

### Unreleased — symbolic links in libraries are no longer followed

- **The scanner never follows a symbolic link below a library root**,
  to a folder or a file, wherever it points. A library organised with
  symlinked series or publisher folders stops indexing them: after the
  first scan on this version those series are reported missing and
  soft-removed, and each link is listed under the library's health
  findings as *Symbolic link skipped*.
- **Those rows are then deleted on the normal schedule.** The removal
  sweeps do not follow the link either, so they see the content as gone:
  a soft-removed row is confirmed once the library's soft-delete window
  has passed (`soft_delete_days`, 30 days by default) and permanently
  deleted, reading progress, markers and collection entries included,
  once the purge window has passed after that (the soft-delete window
  times the `library.hard_purge_multiplier` server setting, 2 by default:
  60 more days, about 90 in all). Setting the multiplier to `0` turns
  the purge off and keeps the rows.
- **What to do instead:** put the real folder, or a bind mount of it
  (`mount --bind`, or a second volume in the compose file), at the
  **same path** inside the library where the link was. The next scan
  then restores the series with its reading history. Adding the link's
  target as a separate library also works for reading, but it creates
  new series there, without that history.
- **The library root itself may still be a link** (`/data/comics ->
  /mnt/x`); only links *below* the root are refused.
- Serving follows the same rule: an issue whose file (or a folder above
  it, below the root) is a link answers not-found. The OPDS download
  also answers not-found for a soft-removed issue.

### Unreleased — a single sign-on login linked to a local account makes it single-sign-on only

Applies only if you turned on **Admin → Auth → Auto-link local accounts by
verified email** (`auth.oidc.link_local_by_verified_email`).

- A link now happens only when the identity provider sends
  `email_verified: true`. An absent claim, even with the trust-unverified
  setting on, no longer links.
- A linked account loses its local password, refresh sessions and app
  passwords (OPDS readers and other apps must be given a new app password),
  whether or not the local address was verified. A disabled local account is
  refused and left unchanged.
- Accounts linked by an earlier release kept their local password. They lose
  it, with their sessions and app passwords, at their next single sign-on;
  the audit log records `auth.oidc.local_password_dropped`. To find them
  beforehand: `SELECT id, email FROM users WHERE external_id LIKE 'oidc:%'
  AND password_hash IS NOT NULL;` and the earlier `auth.oidc.auto_link`
  audit rows.
- Keep a separate local admin account with a different email, so you can
  still sign in when the identity provider is down.

### Unreleased — age-rating caps enforced; library-access `role` removed

- **`library_user_access.age_rating_max` is now enforced.** A user whose
  grant carries a cap no longer sees series or issues rated above it on
  any surface (lists, search, reader, covers, page bytes, OPDS, Komga
  shim). Unrated content stays visible. Caps set before this version
  were stored but ignored — review `/admin/users/{id}` → *Library
  access* before upgrading a family server if you relied on that.
- **The `role` column on `library_user_access` is dropped** (migration
  `m20270123_000001_drop_library_access_role`; `down` restores it as
  `NOT NULL DEFAULT 'reader'`). The `reader | curator` value was never
  enforced. API clients: `LibraryAccessGrantView.role` is gone from
  `GET /api/admin/users/{id}`; `POST /api/admin/users/{id}/library-access`
  gains an optional `age_rating_caps` map (`{ library_id: "Teen" }`,
  values validated against the ComicInfo `AgeRating` ladder → 422
  otherwise) and the grant view returns `age_rating_max`.

### Unreleased — local sign-in is the default without single sign-on

- **`COMIC_AUTH_MODE=local` is no longer needed.** With no sign-in mode
  chosen (no `COMIC_AUTH_MODE`, no saved mode under `/admin/auth`) the
  server uses `local` when none of `COMIC_OIDC_ISSUER` /
  `COMIC_OIDC_CLIENT_ID` / `COMIC_OIDC_CLIENT_SECRET` is set, and `both`
  when all three are. Only some of them set is a boot error naming the
  missing values; an explicit `oidc` / `both` without all three still
  refuses to boot.
- **Existing installs keep their mode.** The first boot of an earlier
  release saved the mode in the database (`auth.mode`), and that row
  still wins, so you can delete the `COMIC_AUTH_MODE=local` line from
  `.env` (which also silences its deprecation WARN) without changing
  anything. A fresh install no longer saves a mode until you pick one
  under `/admin/auth`.

### v0.2.0 — Rust binary becomes the public origin

Before v0.2.0, `compose.prod.yml` published both `app:8080` and
`web:3000` to the host so your reverse proxy could route by path
(`/api`, `/opds` → `app`; everything else → `web`). As of v0.2.0, the
Rust binary handles every request: it serves its own routes (`/api`,
`/opds`, `/auth`, page bytes, WebSockets) directly and reverse-proxies
HTML/RSC/`/_next/*` to the Next.js container internally.

**Pull the new compose and `.env.prod.example` and reconcile your local
copies if you customized them.** (`.env.example` is the development
file.) Two concrete things change:

1. **Drop path-routing from your reverse proxy.** Whether you run
   Caddy, nginx, Traefik, or Kubernetes Ingress, the new template is
   a single upstream pointed at `app:8080`. The path-split rules
   you copied from older versions of `docs/install/*.md` will now
   double-proxy in a confusing way. See the updated templates:
   [caddy](./caddy.md) / [nginx](./nginx.md) / [traefik](./traefik.md)
   / [kubernetes](./kubernetes.md) / [lan-https-mkcert](./lan-https-mkcert.md).

2. **The `COMIC_WEB_BIND` env is no longer honored.** `web` is
   internal-only on the compose bridge. The shipped compose uses
   `expose: ["3000"]` instead of `ports: ["…:3000"]`, so the Next
   port is no longer reachable from the host. If you have something
   on the host that was directly hitting Next (a debugger, a tool
   like `next-devtools`), it now has to go through `app:8080`. You
   can delete the `COMIC_WEB_BIND=…` line from `.env`.

The new `COMIC_WEB_UPSTREAM_URL` env (default `http://web:3000`) is
how the Rust binary finds Next. The shipped compose sets it for you;
override only if you renamed the `web` service.

**Why this changed:** external clients (OPDS readers, OIDC IdP
callbacks, anything not under `/api`) were repeatedly broken because
Next.js was answering for them before the rewrites/middleware were
correctly extended. Making Rust the public origin eliminates that
entire class of routing bug. See
`docs/dev/phase-status.md` and the plan at
`~/.claude/plans/rust-public-origin-1.0.md` for the full background.
