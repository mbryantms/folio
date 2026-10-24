#!/usr/bin/env bash
# Apply the newest production deployment recorded on GitHub.
#
# The release workflow's gated `deploy` job records a GitHub Deployment for
# the `production` environment (see .github/workflows/deploy.yml). This
# script, run by folio-deploy.timer every couple of minutes, reads that
# list anonymously (the repo is public; a conditional request answers 304
# and costs nothing while nothing changed), and when a deployment newer
# than the last one it applied appears, runs folio-deploy.sh for its tag.
#
# Nothing on GitHub can reach this host; the host only ever pulls.
#
# Optional: a GitHub token with `deployments: write` for the repo in
# $FOLIO_DEPLOY_TOKEN_FILE lets the script report success / failure back
# onto the deployment so the GitHub app shows the outcome. Without it the
# outcome is in `journalctl -u folio-deploy` only.
set -euo pipefail

REPO="${FOLIO_REPO:-mbryantms/folio}"
ENV_NAME="${FOLIO_ENVIRONMENT:-production}"
STATE_DIR="${FOLIO_DEPLOY_STATE:-/var/lib/folio-deploy}"
TOKEN_FILE="${FOLIO_DEPLOY_TOKEN_FILE:-/etc/folio-deploy/token}"
DEPLOY="${FOLIO_DEPLOY_SCRIPT:-/opt/folio/bin/folio-deploy.sh}"
# A deployment that fails this many times is left alone (the next one
# supersedes it); a transient failure is retried on the next tick.
MAX_ATTEMPTS="${FOLIO_DEPLOY_MAX_ATTEMPTS:-3}"
API="https://api.github.com/repos/$REPO/deployments?environment=$ENV_NAME&per_page=5"

log() { printf '%s folio-deploy-poll: %s\n' "$(date -Is)" "$*"; }

mkdir -p "$STATE_DIR"
etag_file="$STATE_DIR/etag"
last_file="$STATE_DIR/last-id"
attempts_file="$STATE_DIR/attempts"

hdr=(-H "Accept: application/vnd.github+json" -H "X-GitHub-Api-Version: 2022-11-28" -H "User-Agent: folio-deploy")
if [[ -r "$TOKEN_FILE" ]]; then
  hdr+=(-H "Authorization: Bearer $(tr -d '[:space:]' <"$TOKEN_FILE")")
fi
cond=()
if [[ -s "$etag_file" ]]; then
  cond=(-H "If-None-Match: $(<"$etag_file")")
fi

resp_headers=$(mktemp)
body=$(mktemp)
trap 'rm -f "$resp_headers" "$body"' EXIT
code=$(curl -sS --max-time 20 -D "$resp_headers" -o "$body" -w '%{http_code}' "${hdr[@]}" "${cond[@]}" "$API")
case "$code" in
  304) exit 0 ;;
  200) ;;
  *) log "GitHub answered $code"; exit 1 ;;
esac
grep -i '^etag:' "$resp_headers" | awk '{print $2}' | tr -d '\r' >"$etag_file" || true

# Newest deployment whose ref is a stable release tag.
sel=$(jq -r '[.[] | select(.ref | test("^v[0-9]+\\.[0-9]+\\.[0-9]+$"))] | sort_by(.id) | last
  | if . == null then empty else "\(.id) \(.ref) \(.statuses_url)" end' "$body")
[[ -z "$sel" ]] && exit 0
read -r id ref statuses_url <<<"$sel"
last=$(cat "$last_file" 2>/dev/null || echo 0)
if (( id <= last )); then
  exit 0
fi

attempts=0
if [[ -s "$attempts_file" ]]; then
  read -r a_id a_n <"$attempts_file"
  [[ "$a_id" == "$id" ]] && attempts=$a_n
fi
if (( attempts >= MAX_ATTEMPTS )); then
  exit 0
fi
echo "$id $((attempts + 1))" >"$attempts_file"

post_status() {
  [[ -r "$TOKEN_FILE" ]] || return 0
  curl -sS -o /dev/null --max-time 20 "${hdr[@]}" -X POST "$statuses_url" \
    -d "$(jq -cn --arg s "$1" --arg d "$2" '{state: $s, description: $d}')" || true
}

log "deployment $id → $ref (attempt $((attempts + 1)))"
post_status in_progress "folio-deploy on $(hostname)"
if "$DEPLOY" "$ref"; then
  echo "$id" >"$last_file"
  rm -f "$attempts_file"
  post_status success "deployed $ref"
  log "deployed $ref"
else
  if (( attempts + 1 >= MAX_ATTEMPTS )); then
    post_status failure "deploy of $ref failed $MAX_ATTEMPTS times; see journalctl -u folio-deploy"
    log "giving up on $ref after $MAX_ATTEMPTS attempts"
  else
    post_status error "deploy of $ref failed; retrying"
    log "deploy of $ref failed; retrying on the next tick"
  fi
  exit 1
fi
