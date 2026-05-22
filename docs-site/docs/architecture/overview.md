---
sidebar_position: 1
---

# Architecture overview

Folio is split across a Rust workspace and a Next.js web app, packaged
as two production images.

## Rust workspace

The workspace under [`crates/`](https://github.com/mbryantms/folio/tree/main/crates)
contains six members:

| Crate       | Role                                                     |
| ----------- | -------------------------------------------------------- |
| `server`    | axum HTTP/WebSocket origin; the public entry point.      |
| `entity`    | SeaORM entity definitions.                               |
| `migration` | SeaORM migrations (binary + library).                    |
| `archive`   | CBZ/CBR/CB7/CBT extraction and page indexing.            |
| `parsers`   | ComicInfo.xml, series.json, MetronInfo.xml parsers.      |
| `shared`    | Cross-crate types and helpers.                           |

The toolchain is pinned in
[`rust-toolchain.toml`](https://github.com/mbryantms/folio/blob/main/rust-toolchain.toml)
to channel `1.91.1`.

## Web app

The [`web/`](https://github.com/mbryantms/folio/tree/main/web) workspace
member is a Next.js 16 / React 19 / Tailwind v4 app. It talks to the
Rust server through `apiFetch` (cookie-bound CSRF, `next_cursor`
pagination) and renders the reader, library, and admin surfaces.

## Public origin

In production both processes run side-by-side. The Rust binary is the
**only** public-facing service; the Next.js process listens on the
compose bridge and is reached only through Rust's HTML fallback
upstream. See [Deployment](../operations/deploy) for the topology.
