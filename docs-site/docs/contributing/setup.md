---
sidebar_position: 1
---

# Development setup

The repo uses `just` as the task runner. All commands below are recipes
in [`justfile`](https://github.com/mbryantms/folio/blob/main/justfile).

## Prerequisites

- Rust toolchain — pinned in
  [`rust-toolchain.toml`](https://github.com/mbryantms/folio/blob/main/rust-toolchain.toml)
  to `1.91.1` (rustup auto-installs on first build).
- Node 22+ and pnpm `10.33.2`.
- Docker (for the local Postgres / Redis / Dex services).
- `just` (`cargo install just` or via your package manager).

## First-time setup

```sh
just bootstrap          # tooling check + .env from .env.example
just dev-services-up    # postgres + redis + dex via compose.dev.yml
just migrate            # apply SeaORM migrations
just dev                # cargo run server + pnpm dev web in parallel
```

Then open [http://localhost:8080](http://localhost:8080). The first
user to register becomes the admin.

## Running tests

```sh
just test               # cargo test --workspace + pnpm test
just lint               # clippy + pnpm lint
just fmt-check          # rustfmt + prettier check
```

Server integration tests use `testcontainers` and take ~25 s to warm
up. Run a single file with
`cargo test -p server --test <name>` to iterate.

## Regenerating the OpenAPI spec

```sh
just openapi            # writes web/lib/api/openapi.json + regenerates web types
just openapi-check      # CI guard: fails if the spec drifted
```

`just openapi` is what this docs site's API reference will consume
once Phase B (OpenAPI plugin) is wired in.

## Building this docs site

```sh
just docs               # pnpm --filter docs-site start (localhost:3000)
just docs-build         # production build; fails CI on broken links
```
