# Comic Reader

A self-hostable comic reading platform. Reads `.cbz`, `.cbr`, `.cb7`, `.cbt`, folder-of-images, and pre-paginated EPUB. Parses ComicInfo.xml, series.json, and MetronInfo.xml. Syncs reading progress across devices. Serves OPDS.

See [comic-reader-spec.md](./docs/dev/comic-reader-spec.md) for the full specification.

## Status

Phase 0 — Foundation. Server + web skeleton; auth (OIDC + local) + security middleware in progress.

## Quick start (developers)

```bash
just bootstrap        # one-time tooling + .env setup
just dev-services-up  # postgres + redis + mock OIDC via compose.dev.yml
just migrate          # apply SeaORM migrations
just dev              # run server + web with hot reload
```

Then open <http://localhost:3000> and log in. The first user becomes admin (§12.8 of the spec).

## Quick start (operators)

You need a Linux host with Docker (Compose v2) and an outbound HTTPS path.
The compose stack runs four services: the Rust API (`app`), the Next.js
frontend (`web`), Postgres, and Redis. Both `app` and `web` bind to
loopback only — you put your own reverse proxy in front for TLS.

```bash
# 1. Pick a host directory to hold compose + .env + persistent volumes.
sudo mkdir -p /opt/folio && cd /opt/folio

# 2. Drop in the compose file and the env template.
curl -fsSLO https://raw.githubusercontent.com/mtbry/folio/main/compose.prod.yml
curl -fsSL  https://raw.githubusercontent.com/mtbry/folio/main/.env.example -o .env

# 3. Edit the env file. At minimum: REPO_OWNER, POSTGRES_PASSWORD,
#    COMIC_LIBRARY_HOST_PATH, COMIC_PUBLIC_URL, COMIC_AUTH_MODE.
$EDITOR .env

# 4. Start the stack. First boot runs migrations + generates server secrets.
docker compose -f compose.prod.yml up -d

# 5. Front the loopback ports with TLS. Pick one:
#    - Caddy:    docs/install/caddy.md         (auto Let's Encrypt)
#    - nginx:    docs/install/nginx.md
#    - Traefik:  docs/install/traefik.md
#    - Homelab without a public domain? docs/install/lan-https-mkcert.md
```

Open the URL from `COMIC_PUBLIC_URL` and register — the first user becomes admin.

For upgrades, backups, and the rest of the operator lifecycle, see
[`docs/install/`](./docs/install/).

## Docs

- [Specification](./docs/dev/comic-reader-spec.md)
- [Threat model](./docs/architecture/threat-model.md)
- [Auth model](./docs/architecture/auth-model.md)
- [CSP and security headers](./docs/architecture/csp.md)
- [Rate limits](./docs/architecture/rate-limits.md)
- Reverse proxy templates: [Caddy](./docs/install/caddy.md) · [nginx](./docs/install/nginx.md) · [Traefik](./docs/install/traefik.md)
- [LAN HTTPS via mkcert](./docs/install/lan-https-mkcert.md) (homelab on a private network)
- [Authentik SSO](./docs/install/authentik.md)
- [Upgrades & rollbacks](./docs/install/upgrades.md)
- [Backups](./docs/install/backup.md)
- [Secrets management](./docs/install/secrets-backup.md)
- [Multi-replica scaling](./docs/install/scaling.md)
- [Kubernetes (community)](./docs/install/kubernetes.md)

## Repository layout

See §14 of the spec.

## License

TBD.
