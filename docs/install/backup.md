# Backups

Folio has four kinds of state. Two need backups, one is operator-owned,
one is intentionally ephemeral.

| Surface | Where | Backup? |
|---|---|---|
| **Postgres** — users, libraries, series, issues, progress, markers, sessions, audit log | `comic_postgres` volume | **Yes**, nightly |
| **App data** — secrets, generated thumbnails, search indices | `comic_data` volume (mounted at `/data` in the app container) | **Yes**, weekly (secrets are critical) |
| **Library** — your comic files | host bind mount at `COMIC_LIBRARY_HOST_PATH` | Operator-owned; use whatever backup tool you already use for media |
| **Redis** — job queues, rate-limit counters, ephemeral state | `comic_redis` volume | **No** — restored state would be stale; on Redis loss the app re-enqueues scans on next boot |

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
