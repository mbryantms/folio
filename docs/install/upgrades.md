# Upgrading Folio

Folio ships container images via GHCR. The compose stack you got from
[`README.md`](../../README.md#quick-start-operators) pins both images to
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

# 3. Web is serving HTML.
curl -fsS http://127.0.0.1:3000/sign-in | grep -o '<title>.*</title>'
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

## Multi-replica deployments

When you scale `app` past one replica, set `COMIC_AUTO_MIGRATE=false` and
run migrations as a one-shot before rolling out the new image:

```bash
docker compose -f compose.prod.yml run --rm app /app/migration up
docker compose -f compose.prod.yml up -d --no-deps app
```

See [`scaling.md`](./scaling.md) for the full multi-replica posture.
