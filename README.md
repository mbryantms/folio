# Folio

**A self-hosted comic server and reader for people with real collections.**

[![CI](https://github.com/mbryantms/folio/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/mbryantms/folio/actions/workflows/ci.yml)
[![Latest release](https://img.shields.io/github/v/release/mbryantms/folio)](https://github.com/mbryantms/folio/releases)
[![License: AGPL-3.0-or-later](https://img.shields.io/badge/license-AGPL--3.0--or--later-blue)](./LICENSE)

Folio serves your comic library to any browser, tablet or OPDS app. It is a
single Rust server in front of a modern web reader, built for long runs,
manga, CBL reading lists, archives tagged by ComicTagger a decade ago, and
the occasional slightly broken CBZ that a stricter reader refuses to open.

Your files stay yours. Folio never needs to move or rename them, and with
writeback turned on it stores metadata inside the archives themselves, so
the library stays portable and Folio's database is only a cache.

> **Status:** pre-1.0 and under active development. It is in daily use,
> but expect breaking changes between minor versions. Read the
> [upgrade notes](./docs/install/upgrades.md) before updating.

## What Folio does

### A reader built for comics

- Single-page, double-page and webtoon modes, picked automatically from
  the material and overridable per series.
- True right-to-left manga support, double-page spread awareness and a
  page-strip mini-map.
- Full keyboard and touch control, with every shortcut listed in-app.
- Progress syncs across devices and never moves backwards. Incognito and
  peek modes let you open an issue without touching your history.
- "Up Next" follows your reading list first and the series second.
- Installable as a PWA, with **offline downloads** for a single issue or a
  whole series at a page size you choose.

### A scanner that respects your files

- Reads CBZ, CBR, CB7 and CBT, with JPEG, PNG, WebP, AVIF, GIF and JPEG XL
  pages inside.
- BLAKE3 content hashing gives every issue a stable identity, so retagging
  a file doesn't lose its read state, bookmarks or reading-list matches.
- Repairs two classes of corrupt ZIP that other readers reject, and flags
  encrypted or malformed archives instead of silently skipping them.
- Flat (`Library/Series/`) and publisher-nested layouts, per-library cron
  schedules, and file watching that works on local disks and NAS mounts.
- Deleted files are soft-deleted and restored if they reappear, so an
  unmounted share doesn't wipe your history.

| Format | Support |
|---|---|
| `.cbz` (ZIP) | Read and write |
| `.cbt` (TAR) | Read and write |
| `.cbr` (RAR) | Read; converted to CBZ when edited |
| `.cb7` (7z) | Read; converted to CBZ on scan when the library opts in |

### Metadata matched by cover art

- Fetches from **ComicVine**, **Metron** and the **Grand Comics Database**,
  using your own account or API key for each.
- The primary match signal is the cover itself: perceptual hashes compared
  on ComicTagger's confidence ladder, with title and issue-number matching
  as the fallback.
- A per-field preview before anything is applied, and provenance for every
  field ("set by Metron on May 3rd"). Your manual edits always win.
- Reads ComicInfo.xml, MetronInfo.xml and series.json already in your
  library, so a pre-tagged collection arrives matched.
- Optional **writeback** composes ComicInfo.xml and MetronInfo.xml into the
  archive, so ComicTagger, Komga, Mylar3 and KOReader see the same
  metadata Folio does.
- Handles the case where providers disagree on where one series ends and
  the next begins, without splitting your folders.

### Organizing a collection

- **CBL reading lists** as a first-class feature: import from a file, a
  URL or a built-in catalog of community lists, with three-tier matching,
  refresh history and export back to `.cbl`.
- Collections that hold both series and single issues, a per-user Want to
  Read list, and bookmarks, notes and highlights on any page.
- Saved smart views built from filters, pinned to the home page or to
  custom pages of your own.
- Search across series, issues, people and your own markers.
- Reading stats and a reading log per user.

### Read anywhere

- **OPDS 1.2 and 2.0** with the Page Streaming Extension, so client apps
  stream pages without downloading whole archives.
- Two-way progress sync with Panels (through a Komga-compatible API) and
  KOReader (through its sync-server protocol).
- **Tap-to-copy text** from speech bubbles: server-side OCR with
  comic-aware bubble detection, in English and Japanese.

### Multi-user and self-hosting

- Per-library access control enforced on every surface, including search
  results, OPDS feeds and page images.
- Local accounts and OIDC single sign-on (Authentik, Dex and others), plus
  revocable per-app passwords for OPDS clients.
- An admin area for users, libraries, scans, background work, job queues,
  logs and an append-only audit log. Mail, auth policy and worker settings
  are edited in the UI without a restart.
- One Docker Compose stack with a single public port, Prometheus metrics,
  and reverse-proxy templates for Caddy, nginx and Traefik.

### Fast

Pages stream straight out of the archive with no extraction step, and all
heavy work (scanning, thumbnails, metadata, archive rewrites) runs in
background workers. On a developer workstation with NVMe storage, a cold
scan of a 67 GB, 1,395-issue library takes about 15 seconds, and a
re-check with nothing changed takes about half a second.

The full feature tour, with how each part works, is in
[docs/features.md](./docs/features.md).

## Getting started

You need a Linux host with Docker and Compose v2. Folio listens on
`127.0.0.1:8080` and expects a reverse proxy in front of it for TLS.

```bash
mkdir -p ~/folio && cd ~/folio
curl -fsSLO https://raw.githubusercontent.com/mbryantms/folio/main/compose.prod.yml
curl -fsSL  https://raw.githubusercontent.com/mbryantms/folio/main/.env.example -o .env

# Set REPO_OWNER=mbryantms, POSTGRES_PASSWORD, COMIC_LIBRARY_HOST_PATH
# and COMIC_PUBLIC_URL, then:
$EDITOR .env
docker compose -f compose.prod.yml up -d
```

Open the address you set as `COMIC_PUBLIC_URL` and register. The first
account becomes the administrator. Migrations run and server secrets are
generated on first boot.

Next steps, including reverse-proxy setup, single sign-on, backups and
upgrades, are in [docs/install/](./docs/install/).

## Documentation

- [Feature tour](./docs/features.md)
- [Installation and operations](./docs/install/): reverse proxies, SSO,
  backups, upgrades, scaling
- [Architecture and security](./docs/architecture/): auth model, threat
  model, CSP, rate limits
- [Developer docs](./docs/dev/): specification, scanner, metadata
  pipeline, OPDS
- [Changelog](./CHANGELOG.md)

## Development

Folio is a Rust workspace (axum, SeaORM, Postgres, Redis) behind a
Next.js web app. You need Docker, [just](https://github.com/casey/just),
Node 24 with pnpm, and the Rust toolchain pinned in `rust-toolchain.toml`.

```bash
just bootstrap         # one-time tooling and .env setup
just dev-services-up   # Postgres, Redis and a mock OIDC provider
just migrate
just dev               # server and web app with hot reload
```

Then open <http://localhost:8080>. `just test` runs the Rust and web test
suites.

## Contributing

Bug reports and feature requests are welcome in
[GitHub Issues](https://github.com/mbryantms/folio/issues). For a code
change beyond a small fix, please open an issue first so the approach can
be agreed before you invest the time. Pull requests need to pass
`just test` and the linters that CI runs.

## Security

Please do not report security vulnerabilities in public issues. Use
[private vulnerability reporting](https://github.com/mbryantms/folio/security/advisories/new)
instead.

## License

Folio is licensed under the [GNU Affero General Public License v3.0 or
later](./LICENSE). If you run a modified version as a network service,
the AGPL requires you to offer its source to that service's users.

Third-party notices, including the terms for the bundled `unrar` binary,
are in [LICENSE-THIRD-PARTY.md](./LICENSE-THIRD-PARTY.md).

Folio is not affiliated with ComicVine, Metron or the Grand Comics
Database. Metadata from those services is subject to their own terms;
Grand Comics Database data is licensed under CC BY-SA 4.0.
