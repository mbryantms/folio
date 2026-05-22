---
sidebar_position: 1
slug: /intro
---

# Folio

Folio is a self-hostable comic reader. The codebase is a Rust
workspace backing a Next.js web app, deployed as two containers
behind an operator-owned reverse proxy.

## What this site covers

- **[Features](./category/features)** — image-rich spotlight on every
  user-facing feature: reader, library, markers, collections, saved
  views, search, reading log, OPDS clients, account, admin.
- **[Architecture](./architecture/overview)** — how the Rust crates
  and the web app fit together; reference docs for the library
  scanner, OCR pipeline, multi-select / bulk-action contract, and
  the long-form application specification.
- **[Operations](./operations/deploy)** — how to deploy, configure
  (env + admin UI), back up, and upgrade Folio.
- **[Contributing](./contributing/setup)** — getting a development
  environment running, plus design conventions like the cover-card
  corner cascade.
- **[API](./api/comic-reader-api)** — endpoint reference generated from the live
  utoipa-emitted OpenAPI spec (138 endpoints; regenerated on every
  build).
- **[Other references](./references)** — pointers into the repo for
  docs that haven't been promoted: snapshot audits, deep operator
  references, per-client OPDS compatibility notes.

## Conventions

Every page in this site has been verified against the repo at
promotion time. Reference docs that move faster than this site can
keep up with (per-client OPDS quirks, point-in-time audits) are kept
in the repo and linked from [Other references](./references) rather
than copied in.
