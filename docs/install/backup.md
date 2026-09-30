# Backups

Folio has four kinds of state. Two need backups, one is operator-owned,
one is intentionally ephemeral.

| Surface | Where | Backup? |
|---|---|---|
| **Postgres** — users, libraries, series, issues, progress, markers, sessions, audit log | `comic_postgres` volume | **Yes**, nightly |
| **App data** — secrets, generated thumbnails, search indices | `comic_data` volume (mounted at `/data` in the app container) | **Yes**, weekly (secrets are critical) |
| **Library** — your comic files | host bind mount at `COMIC_LIBRARY_HOST_PATH` | Operator-owned; use whatever backup tool you already use for media |
| **Redis** — job queues, rate-limit counters, ephemeral state | `comic_redis` volume | **No** — restored state would be stale. Losing Redis loses whatever was queued (scans, thumbnails, metadata jobs); the app does **not** re-enqueue that work at boot. See [Redis loss](#redis-loss) below. |

## Redis loss

Redis holds the apalis job queues and nothing durable. If the volume is
lost or wiped:

- **Queued and in-flight jobs are gone.** Boot only reaps orphan
  `.tmp` files left by interrupted archive rewrites and sweeps stale
  scan-coalescing keys; it does not reconstruct or re-enqueue lost
  scan, thumbnail, or metadata jobs
  ([`app.rs`](../../crates/server/src/app.rs), the startup-cleanup block).
- **Scheduled scans resume on their own** at the next
  `scan_schedule_cron` tick for each library — the schedule lives in
  Postgres, not Redis.
- **Trigger a manual scan** of each library from `/admin/libraries`
  (or `POST /api/libraries/{slug}/scan`) after restoring Redis if you
  don't want to wait for the cron. A scan re-enqueues the post-scan
  thumbnail and search work for anything it finds changed; for
  thumbnails on unchanged issues use the "Generate missing" action on
  the admin thumbnails page.
- Rate-limit counters and auth WebSocket tickets simply reset; users
  may need to reload an open reader tab.

## Postgres — nightly

`pg_dump` against the running Postgres container is the supported path.
It's safe to run while the app is up; Postgres writes a consistent
snapshot.

```bash
mkdir -p /var/backups/folio
docker compose -f /opt/folio/compose.prod.yml exec -T postgres \
  pg_dump -U comic -Fc comic_reader \
  > /var/backups/folio/postgres-$(date +%F).dump
```

`-Fc` writes the custom format — smaller than plain SQL, faster to
restore, and `pg_restore` can selectively skip tables (useful for
restoring just the auth tables after a botched migration).

Restore is the inverse:

```bash
docker compose -f /opt/folio/compose.prod.yml exec -T postgres \
  pg_restore --clean --if-exists -U comic -d comic_reader \
  < /var/backups/folio/postgres-2026-05-12.dump
```

For plain-SQL dumps (`-Fp`), use `psql` to restore — see
[`upgrades.md`](./upgrades.md) for the gunzip-into-psql one-liner.

## App data — weekly

The `comic_data` volume holds three things, in descending order of
importance:

1. `/data/secrets/` — JWT key, password pepper, email-token HMAC,
   URL-signing HMAC. **Losing these invalidates every session, every
   outstanding password-reset link, and every signed page-streaming
   URL.** Back these up. See [`secrets-backup.md`](./secrets-backup.md)
   for the full failure mode.
2. `/data/thumbs/` — generated cover + page thumbnails. Regenerable
   (the post-scan worker rebuilds them when missing), but rebuilding is
   slow for a large library.
3. `/data/search/` — full-text search indices. Also regenerable but
   slow to rebuild.

The whole volume is small enough (<5 GB for most libraries) that a
weekly tar is the simplest path:

```bash
docker run --rm \
  -v folio_comic_data:/d \
  -v /var/backups/folio:/b \
  alpine \
  tar czf /b/data-$(date +%F).tgz -C /d .
```

Restore:

```bash
docker compose -f /opt/folio/compose.prod.yml down app
docker run --rm \
  -v folio_comic_data:/d \
  -v /var/backups/folio:/b \
  alpine \
  sh -c 'rm -rf /d/* /d/.[!.]* && tar xzf /b/data-2026-05-12.tgz -C /d'
docker compose -f /opt/folio/compose.prod.yml up -d app
```

## `just backup` / `just restore`

The two commands above are wrapped as `just` recipes for anyone running
the stack from a checkout (or with `just` installed next to
`compose.prod.yml`):

```bash
# pg_dump -Fc + comic_data tar, both timestamped, into $BACKUP_DIR
# (default ./backups). Safe while the app is running.
just backup
BACKUP_DIR=/var/backups/folio just backup

# Restore a dump from `just backup` into the running compose postgres.
# Stops `app` for the duration, asks you to type `restore` to confirm,
# runs `pg_restore --clean --if-exists`, then starts `app` again.
just restore ./backups/postgres-20260929T031500Z.dump
```

Both take an optional `compose=<file>` (default `compose.prod.yml`) and
locate the `comic_data` volume from the running `postgres` container's
compose project label, so they work whatever project name compose chose.
`just restore` refuses a `.tgz` — the data-volume restore stays the
manual `docker run … tar xzf` sequence above because it has to run with
`app` down and replaces `secrets/` (see
[`secrets-backup.md`](./secrets-backup.md) before restoring one).

## A backup script

A reference `scripts/backup.sh` lives in the repo at
[`scripts/backup.sh`](../../scripts/backup.sh). It does the Postgres
dump + `comic_data` tar in one shot and is safe to run from cron:

```cron
# /etc/cron.d/folio-backup
15 3 * * *  root  /opt/folio/scripts/backup.sh nightly
15 3 * * 0  root  /opt/folio/scripts/backup.sh weekly
```

Off-host (S3 / Backblaze / a NAS) replication is the operator's call;
add a `rclone` step after the local backup or back up `/var/backups/folio/`
with your existing host-level tool.

## Retention

Folio has no opinion. A common starter policy:

| Tier    | Keep |
|---------|------|
| nightly Postgres dump | 14 days |
| weekly `comic_data` tar | 8 weeks |
| monthly off-host copy | 12 months |

Old enough to ride out a "we noticed last month that…" issue, fresh
enough to not eat disk.

## Verifying a backup

Once a quarter, restore the most recent nightly into a throwaway compose
project and confirm `/readyz` returns 200. Untested backups aren't
backups.

```bash
cp -r /opt/folio /tmp/folio-restore-test
cd /tmp/folio-restore-test
# Change image tags + ports to avoid colliding with the running stack.
# Restore the Postgres dump into the new compose's postgres container.
# Bring it up, hit /readyz, confirm the user count + a recent issue.
```

## What backups don't cover

- **The library itself** — `COMIC_LIBRARY_HOST_PATH` is a host bind
  mount. Back it up with the same tool you use for other large media
  collections (restic, rsnapshot, ZFS snapshots).
- **Operator-facing config** — your `.env`, your reverse-proxy config,
  your TLS certs. These live outside the volumes; back up `/opt/folio/`
  as a tree.
- **A per-user, app-level dump.** A Postgres dump restores the whole
  server, but it is tied to this instance's issue ids: re-import the
  library after a retag and the restored progress may point at rows
  that no longer exist. Each user can download their own data —
  progress, notes and bookmarks, collections, saved views, ratings,
  pages, sidebar, reading log, preferences — as one JSON file from
  **Settings → Account → Export my data** (`GET /api/me/export`).
  Every issue in it is keyed by content hash and series name / year /
  number, so it stays readable after a rebuild or on another host. The
  shape is documented in [`docs/dev/export-format.md`](../dev/export-format.md).
  There is no importer; it is a durable copy, not a restore path.
