# syntax=docker/dockerfile:1.7
# Folio app image — Rust workspace (axum server + migration runner).
#
# Multi-stage:
#   1a/1b. cargo-chef plan + cached cook for the Rust workspace
#   2.    Pulls `tini` + `unrar-free` out of a slim Debian intermediate
#   3.    Distroless final image — only the two binaries + tini + unrar
#
# The Next.js frontend lives in a separate image — see `web/Dockerfile`.
# Production runs them as two compose services fronted by an operator-owned
# reverse proxy. See `docs/install/` for the wiring.

# ───── Stage 1a: cargo-chef recipe ─────
FROM rust:1.91-slim-bookworm AS planner
WORKDIR /work
RUN cargo install cargo-chef --locked
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates
RUN cargo chef prepare --recipe-path recipe.json

# ───── Stage 1b: cargo-chef cook (cached deps) ─────
FROM rust:1.91-slim-bookworm AS rust-builder
WORKDIR /work
# build-essential / g++ pulled in for cc-rs crates (zstd-sys, image, webp,
# blake3, etc.) that compile C/C++. pkg-config + libssl-dev cover the
# native OpenSSL link path used by reqwest's default features.
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*
RUN cargo install cargo-chef --locked
COPY --from=planner /work/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json --bin server --bin migration

# Build-time fingerprints. The `.git` directory is NOT in the Docker
# context, so crates/server/build.rs can't shell out to git from inside
# the container. CI passes these values as --build-arg; the build script
# picks them up via env. See `.github/workflows/release.yml` for the
# producer side. Defaults keep local `docker build` runnable without
# args (image identifies as "dev").
ARG COMIC_BUILD_TAG=dev
ARG COMIC_BUILD_SHA=unknown
ARG COMIC_BUILD_SHA_FULL=unknown
ARG COMIC_BUILD_REPO_URL=
ENV COMIC_BUILD_TAG=$COMIC_BUILD_TAG \
    COMIC_BUILD_SHA=$COMIC_BUILD_SHA \
    COMIC_BUILD_SHA_FULL=$COMIC_BUILD_SHA_FULL \
    COMIC_BUILD_REPO_URL=$COMIC_BUILD_REPO_URL

COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates
RUN cargo build --release --bin server --bin migration \
    && strip /work/target/release/server /work/target/release/migration

# ───── Stage 2: tini + unrar from Debian apt ─────
FROM debian:bookworm-slim AS apt-source
RUN apt-get update && apt-get install -y --no-install-recommends \
    unrar-free tini \
    && rm -rf /var/lib/apt/lists/*

# ───── Stage 3: distroless runtime ─────
# distroless/cc carries glibc + libssl; required by the Rust binary (reqwest,
# argon2, sea-orm Postgres TLS) and by unrar-free.
FROM gcr.io/distroless/cc-debian12:nonroot AS runtime
WORKDIR /app

# Re-declare build-time args in the final stage so the LABEL block below
# can interpolate them. Docker scopes ARGs per-stage; values come from
# the same `--build-arg` flags CI passes for the rust-builder stage.
ARG COMIC_BUILD_TAG=dev
ARG COMIC_BUILD_SHA_FULL=unknown
ARG COMIC_BUILD_REPO_URL=https://github.com/mbryantms/folio

# OCI labels — `org.opencontainers.image.source` is what GHCR uses to link
# the image package back to the repo; `.version` + `.revision` make the
# image self-describing for `docker inspect` and supply-chain scanners.
LABEL org.opencontainers.image.title="Folio" \
      org.opencontainers.image.description="Self-hostable comic reader (Rust server)" \
      org.opencontainers.image.source="${COMIC_BUILD_REPO_URL}" \
      org.opencontainers.image.version="${COMIC_BUILD_TAG}" \
      org.opencontainers.image.revision="${COMIC_BUILD_SHA_FULL}" \
      org.opencontainers.image.licenses="AGPL-3.0-or-later"

# Binaries
COPY --from=rust-builder /work/target/release/server    /app/server
COPY --from=rust-builder /work/target/release/migration /app/migration

# unrar (binary, GPL; see LICENSE-THIRD-PARTY.md)
COPY --from=apt-source   /usr/bin/unrar-free            /usr/bin/unrar

# tini as PID 1 — reaps zombies, forwards SIGTERM to the server for graceful drain.
COPY --from=apt-source   /usr/bin/tini                  /sbin/tini

VOLUME ["/library", "/data"]
EXPOSE 8080

ENV COMIC_BIND_ADDR=0.0.0.0:8080 \
    COMIC_LIBRARY_PATH=/library \
    COMIC_DATA_PATH=/data \
    COMIC_AUTO_MIGRATE=true

# Healthcheck handled by the orchestrator via `/app/server --healthcheck`
# (see `compose.prod.yml`). Inline `HEALTHCHECK` would bake the cadence into
# the image, which is the operator's call, not ours.

ENTRYPOINT ["/sbin/tini", "--", "/app/server"]
