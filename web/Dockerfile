# syntax=docker/dockerfile:1.7
# Folio web image — Next.js 16 standalone bundle on distroless Node.
#
# Multi-stage:
#   1. pnpm install + `next build` (which writes the standalone server.js + .next/static)
#   2. Distroless nodejs runtime, copy only the standalone bundle + static + public
#
# Pairs with the Rust `app` image (see root `Dockerfile`). Production runs
# both as compose services fronted by an operator-owned reverse proxy.

# ───── Stage 1: build the standalone bundle ─────
FROM node:22-bookworm-slim AS builder
WORKDIR /work

# pnpm is fast + matches what the repo uses locally. Pinning the major to
# match `package.json/packageManager` keeps reproducibility tight.
RUN npm install -g pnpm@10

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
# `nodejs22-debian12:nonroot` ships a hardened Node with the same posture as
# the Rust image's `cc-debian12:nonroot`. No shell, no package manager, runs
# as a non-root user.
FROM gcr.io/distroless/nodejs22-debian12:nonroot AS runtime
WORKDIR /app

LABEL org.opencontainers.image.title="Folio Web" \
      org.opencontainers.image.description="Self-hostable comic reader (Next.js frontend)" \
      org.opencontainers.image.source="https://github.com/mtbry/folio" \
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
