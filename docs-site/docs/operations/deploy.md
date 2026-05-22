---
sidebar_position: 1
---

# Deployment

Folio ships as two container images:

- `ghcr.io/<your-org>/folio` — the Rust origin (axum + sea-orm + apalis workers).
- `ghcr.io/<your-org>/folio-web` — the Next.js SSR upstream.

In production both run side-by-side behind an operator-owned reverse
proxy. The Rust binary is the **only public-facing service**; the Next
process is internal-only on the compose bridge and reached through
Rust's HTML fallback (`COMIC_WEB_UPSTREAM_URL`).

## Compose

The reference deployment lives at
[`compose.prod.yml`](https://github.com/mbryantms/folio/blob/main/compose.prod.yml)
and brings up four services:

| Service     | Role                                       | Exposure         |
| ----------- | ------------------------------------------ | ---------------- |
| `app`       | Rust origin (HTTP + WebSocket).            | `127.0.0.1:8080` by default |
| `web`       | Next.js SSR upstream.                      | Internal only.   |
| `postgres`  | Schema, user data, sessions.               | Internal only.   |
| `redis`     | apalis job queue, rate-limit counters.     | Internal only.   |

TLS termination is the operator's responsibility. Reverse-proxy
templates ship under
[`docs/install/`](https://github.com/mbryantms/folio/tree/main/docs/install):
Caddy, nginx, Traefik, plus a Cloudflare-front note and an
mkcert-based homelab variant.

## Required environment

The minimum set required by `compose.prod.yml` (see the inline header
of that file for the full list):

| Variable                    | Purpose                                        |
| --------------------------- | ---------------------------------------------- |
| `REPO_OWNER`                | GHCR namespace the images are pulled from.     |
| `POSTGRES_PASSWORD`         | Postgres password for the `comic` user.        |
| `COMIC_LIBRARY_HOST_PATH`   | Absolute host path to the comic library.       |
| `COMIC_PUBLIC_URL`          | Public HTTPS URL (e.g. `https://comics.example.com`). |

`COMIC_APP_BIND=0.0.0.0` exposes the API port directly on the host's
LAN interface (for homelab setups without a reverse proxy). Do **not**
set this on a public-internet host — the binary terminates HTTP, not
HTTPS.

## Backups and upgrades

See the install guides:

- [`docs/install/backup.md`](https://github.com/mbryantms/folio/blob/main/docs/install/backup.md)
- [`docs/install/upgrades.md`](https://github.com/mbryantms/folio/blob/main/docs/install/upgrades.md)
- [`docs/install/secrets-backup.md`](https://github.com/mbryantms/folio/blob/main/docs/install/secrets-backup.md)

These reference docs have not yet been re-verified for the docs site
and are linked out to the repo as the canonical copy.
