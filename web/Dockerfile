# syntax=docker/dockerfile:1.27@sha256:bde3983e9c939224420ddaf6b784cc30e09b035a4dea01f581230c50809f372e
# Folio web image — Next.js 16 standalone bundle on distroless Node.
#
# Multi-stage:
#   1. pnpm install + `next build` (which writes the standalone server.js + .next/static)
#   2. Distroless nodejs runtime, copy only the standalone bundle + static + public
#
# Base images are pinned by digest (tag@sha256:...). Renovate refreshes the
# digest whenever upstream rebuilds the tag (OS package fixes) and
# auto-merges once CI boots the result; a TAG change (new Debian/Node/Rust
# line) still waits for review. See docs/dev/dependency-management.md.
#
# Pairs with the Rust `app` image (see root `Dockerfile`). Production runs
# both as compose services fronted by an operator-owned reverse proxy.

# ───── Stage 1: build the standalone bundle ─────
FROM node:24-bookworm-slim@sha256:2fe369e969550cde8e867afc3fe370b260140cab4a23d467074295b42163d553 AS builder
WORKDIR /work

# pnpm is fast + matches what the repo uses locally. The exact version is
# kept in lockstep with `package.json/packageManager` by Renovate (see the
# `pnpm` custom manager + group in renovate.json).
RUN npm install -g pnpm@12.4.2

# Bring in just enough to make pnpm install efficient — workspace manifest,
# lockfile, and the web package contents. The root `Cargo.toml` etc. live
# outside the build context per `web/.dockerignore`.
COPY pnpm-workspace.yaml pnpm-lock.yaml package.json ./
COPY web ./web

RUN cd web && pnpm install --frozen-lockfile

# `API_PROXY_URL` is captured by next.config.ts's `rewrites()` and baked
# into `.next/routes-manifest.json` at build time. The compose-internal
# default targets the `app` service on the compose bridge network; pass
# `--build-arg API_PROXY_URL=…` to override for a different deployment
# topology (e.g. a sidecar / host-network setup). NOT a `NEXT_PUBLIC_*`
# var — this hostname is server-only and must not be inlined into the
# client bundle.
ARG API_PROXY_URL=http://app:8080
ENV API_PROXY_URL=${API_PROXY_URL}
RUN cd web && pnpm build

# ───── Stage 2: distroless Node runtime ─────
# `nodejs24-debian13:nonroot` ships a hardened Node with the same posture as
# the Rust image's `cc-debian13:nonroot`. No shell, no package manager, runs
# as a non-root user. debian13 (trixie) rather than debian12: the bookworm
# tag carried a fixable CRITICAL + 5 HIGH libssl3 CVEs for weeks (2026-09)
# while the trixie tag was clean. The standalone bundle is JS + prebuilt
# native modules and runs on the newer glibc unchanged (docker-smoke boots it).
FROM gcr.io/distroless/nodejs24-debian13:nonroot@sha256:7781e8b4fccf59240bd539af6738cccf8dad4be303165c3a1fa065c48699b937 AS runtime
WORKDIR /app

LABEL org.opencontainers.image.title="Folio Web" \
      org.opencontainers.image.description="Self-hostable comic reader (Next.js frontend)" \
      org.opencontainers.image.source="https://github.com/mbryantms/folio" \
      org.opencontainers.image.licenses="AGPL-3.0-or-later"

# Next.js standalone layout:
#   /app/server.js          (the entry — Next 16's minimal Node server)
#   /app/.next/static/      (immutable hashed assets)
#   /app/public/            (user-managed static files)
# The standalone output already includes a trimmed `node_modules/`.
COPY --from=builder /work/web/.next/standalone /app
COPY --from=builder /work/web/.next/static     /app/web/.next/static
COPY --from=builder /work/web/public           /app/web/public

ENV NODE_ENV=production \
    NEXT_TELEMETRY_DISABLED=1 \
    HOSTNAME=0.0.0.0 \
    PORT=3000

EXPOSE 3000

# distroless-nodejs's ENTRYPOINT is `/nodejs/bin/node`. Pass the standalone
# server entrypoint as CMD; the standalone bundle puts it at /app/web/server.js
# (path depends on how Next mirrors the workspace into the bundle).
CMD ["web/server.js"]
