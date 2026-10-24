#!/usr/bin/env bash
# Pull, recreate and prune the Folio compose stack at one release tag, then
# verify the running server reports that version. Run by
# folio-deploy-poll.sh when a production deployment is recorded on GitHub,
# or by hand: `folio-deploy.sh v1.2.3`.
set -euo pipefail

VERSION="${1:?usage: folio-deploy.sh vX.Y.Z}"
COMPOSE_DIR="${FOLIO_COMPOSE_DIR:-/opt/folio}"
HEALTH_URL="${FOLIO_HEALTH_URL:-http://127.0.0.1:8080/healthz}"
# How long to wait for /healthz to report the new version (5 s per try).
HEALTH_TRIES="${FOLIO_HEALTH_TRIES:-60}"

log() { printf '%s folio-deploy: %s\n' "$(date -Is)" "$*"; }

cd "$COMPOSE_DIR"
# The compose file reads `${TAG:-latest}`; an exported TAG wins over .env, so
# the containers run the exact release and the image label says which.
export TAG="$VERSION"

log "pulling images for $VERSION"
docker compose pull --quiet
log "recreating containers"
docker compose up -d --remove-orphans

log "waiting for $HEALTH_URL to report $VERSION"
running=""
for _ in $(seq 1 "$HEALTH_TRIES"); do
  body=$(curl -fsS --max-time 5 "$HEALTH_URL" 2>/dev/null || true)
  running=$(printf '%s' "$body" | jq -r '.version // empty' 2>/dev/null || true)
  if [[ "$running" == "$VERSION" ]]; then
    log "healthy, running $running"
    # Old images, stopped containers, unused networks, build cache. Volumes
    # are never touched (no --volumes).
    docker system prune -af >/dev/null
    log "pruned unused docker objects"
    docker compose ps
    exit 0
  fi
  sleep 5
done
log "ERROR: server did not report $VERSION (last seen: ${running:-nothing})"
docker compose ps
exit 1
