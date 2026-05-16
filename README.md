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

Then open <http://localhost:8080> and log in. The first user becomes admin (§12.8 of the spec).

`:8080` is the Rust binary; it serves API routes itself and reverse-proxies HTML / RSC / `/_next/*` assets (including the HMR WebSocket) to the Next dev server at `:3000` internally. Hitting `:3000` directly works for raw Next debugging but bypasses the Rust auth/CSRF/security-headers stack, so it isn't the supported entry point.

HMR smoke test: load <http://localhost:8080>, then edit any file under `web/app/` or `web/components/` and watch the browser update without a manual reload. The HMR signal flows over WebSocket through the same proxy path the v0.2 cutover added (`upstream::proxy_websocket`); a broken HMR usually means the Next dev server isn't running or `:3000` is bound to a different process.

## Quick start (operators)

You need a Linux host with Docker (Compose v2) and an outbound HTTPS path.
The compose stack runs four services:

- **`app`** — Rust binary. The public-facing service: serves the API, OPDS, WebSockets, page bytes, and reverse-proxies HTML + RSC + `/_next/*` assets to the internal Next.js upstream. Bound to `127.0.0.1:8080` by default; put your reverse proxy in front for TLS.
- **`web`** — Next.js SSR upstream, internal-only on the compose bridge. Not published to the host.
- **`postgres`** + **`redis`** — also internal-only.

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

# 5. Front `app:8080` with TLS. Single upstream — the reverse proxy
#    does NOT need path-based routing; it just forwards everything
#    to the Rust binary. Templates:
#    - Caddy:    docs/install/caddy.md         (auto Let's Encrypt)
#    - nginx:    docs/install/nginx.md
#    - Traefik:  docs/install/traefik.md
#    - Homelab without a public domain? docs/install/lan-https-mkcert.md
```

Open the URL from `COMIC_PUBLIC_URL` and register — the first user becomes admin.

**Upgrading from v0.1.x?** Read [`docs/install/upgrades.md#v020--rust-binary-becomes-the-public-origin`](./docs/install/upgrades.md) first — your reverse-proxy config needs a one-time simplification (drop the path-routing rules) and the `web` container loses its host port binding.

For backups, scaling, and the rest of the operator lifecycle, see [`docs/install/`](./docs/install/).

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
