# Scaling beyond a single instance

Comic Reader v1 is designed for single-instance deployments. Most homelabs and small-family installs will never need more. This doc covers what to know if you do.

## Multi-replica

> **CRITICAL:** When running >1 replica, set `COMIC_AUTO_MIGRATE=false`.

Two instances racing migrations corrupts schema. Run migrations once from a one-shot init container before bringing up replicas:

```yaml
services:
  migrate:
    image: comic-reader:latest
    command: ["/app/migration", "up"]
    environment:
      COMIC_DATABASE_URL: ${COMIC_DATABASE_URL}
    restart: "no"

  app:
    image: comic-reader:latest
    deploy:
      replicas: 3
    environment:
      COMIC_AUTO_MIGRATE: "false"
    depends_on:
      migrate:
        condition: service_completed_successfully
```

## Sticky sessions

Not required. The cookie-session JWT is stateless; refresh-token rotation is a single Postgres transaction.

WebSocket connections are session-affine to whichever instance accepted the upgrade. The sync layer publishes change events through Redis pub/sub so clients on different instances stay in sync — no special LB configuration needed.

## Postgres

A single Postgres instance comfortably serves the largest realistic install (≤100k issues, ≤50 concurrent readers — see §18). Beyond that:

- Read replicas: not implemented in v1. Would need read/write split logic in SeaORM.
- Connection pool: app default `max_connections=30`; for >2 replicas, lower per-replica or move to PgBouncer.

## Redis

Single Redis instance per deployment. The app uses Redis for: rate limiting, sync pub/sub, apalis job queue, WebSocket auth tickets, search dictionary cache. Sentinel/Cluster modes are not tested in v1.

## Library mount

Multi-replica setups must share the library mount across instances. NFS works but `notify` events are unreliable (§E3) — schedule a periodic full rescan as fallback.

## When to consider splitting services

Only when:
- The single-binary CPU is dominated by scan work and reader latency suffers, **and**
- Vertical scaling is exhausted.

Then: extract the scanner into a separate worker that consumes the same apalis queue. The architecture (§2 of the spec) was deliberately designed to make this a small refactor.
