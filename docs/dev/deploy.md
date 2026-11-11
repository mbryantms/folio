# Deploying a release to the compose host

The production instance runs the two GHCR images with Docker Compose in an
LXC (`/opt/folio`, `docker-compose.yml`, `TAG=latest` in `.env`). A release
reaches it without anyone logging into the host:

1. **Merge the release PR** (release-please) — from the GitHub app is fine.
   The tag triggers `release.yml`, which publishes both images.
2. **Approve the deployment.** `release.yml` ends with the `deploy` job
   (`deploy.yml`), whose job runs in the `production` environment. The
   environment's required reviewers get a "Review deployments" notification;
   approving it in the GitHub app is the deploy button. Without reviewers
   the job runs on its own.
3. **The job records a GitHub Deployment** for the tag (after checking both
   images are on GHCR). It never reaches the host.
4. **The host applies it.** `folio-deploy.timer` runs
   `folio-deploy-poll.sh` every two minutes. It reads the repo's production
   deployments anonymously (public repo; a conditional request answers 304
   while nothing changed) and, for a deployment newer than the last one it
   applied, runs `folio-deploy.sh <tag>`: `docker compose pull`,
   `docker compose up -d --remove-orphans` with `TAG=<tag>`, waits for
   `/healthz` to report the tag, then `docker system prune -af` (volumes
   untouched). A failed deploy is retried on the next tick, up to three
   times, then left for the next deployment to supersede.

The host's compose file is its own copy. A deployment changes only the
image tag, so an image change reaches production with the next release
while a change to [`compose.prod.yml`](../../compose.prod.yml) reaches it
only when someone copies it to `/opt/folio/docker-compose.yml` (keeping
the host's local edits) and runs `docker compose up -d`. Image changes
therefore must keep working with the compose file an existing install
already has: no new required variable, mount, tmpfs or capability. The
template's hardening (read-only roots, dropped capabilities,
`no-new-privileges`, the internal `backend` network, the optional
`REDIS_PASSWORD`) and the steps to adopt it on an existing host are in
[`docs/install/upgrades.md`](../install/upgrades.md) under
*Unreleased — hardened compose template*. `compose.test.yml`, which the
`docker-smoke` CI job boots, carries the same app and web hardening, so a
published image is known to start under it.

Re-deploy or roll back: run `deploy.yml` by hand (Actions → deploy → Run
workflow, also from the GitHub app) with any published tag. The host
treats it like any other deployment.

Files: [`deploy/host/`](../../deploy/host/) (scripts, units, install
steps), [`.github/workflows/deploy.yml`](../../.github/workflows/deploy.yml).

Watching: `systemctl list-timers folio-deploy.timer`,
`journalctl -u folio-deploy -n 50`. State in `/var/lib/folio-deploy/`
(`last-id`, `etag`, `attempts`); delete `last-id` to make the host re-apply
the newest deployment.

Outcome on GitHub: the deployment stays "pending" on the GitHub side unless
the host can report back. A fine-grained token with *Deployments: write*
on the repo in `/etc/folio-deploy/token` (mode 0600) makes the poller post
`in_progress` / `success` / `failure` statuses, which the GitHub app shows.
Nothing else needs a secret.

Why pull-based: the host accepts no inbound connections and holds no
credentials; GitHub can only *announce* a deployment, and only someone with
write access to the repo can do that.
