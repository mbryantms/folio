# Traefik reverse proxy

If you already run [Traefik](https://traefik.io/) on the same docker
host, the simplest integration is to attach labels to the `app` and
`web` services in `compose.prod.yml`. Traefik picks them up via its
docker provider and routes traffic without any nginx-style vhost file.

This page assumes:

- Traefik v3 running with the docker provider enabled.
- An external docker network (named `proxy` here) that both Traefik and
  Folio attach to.
- A certificate resolver named `le` doing Let's Encrypt over HTTP-01 or
  DNS-01.

See [Caddy](./caddy.md) or [nginx](./nginx.md) if you'd rather not run
Traefik.

## Labels overlay

Drop these labels onto the `app` and `web` services in
`compose.prod.yml` (or, cleaner, layer them in via a
`compose.traefik.yml` override). The two services bind to loopback by
default — for Traefik discovery we instead expose them on the shared
proxy network and drop the host port bind.

```yaml
# compose.traefik.yml
networks:
  proxy:
    external: true

services:
  app:
    networks: [default, proxy]
    ports: !reset []
    labels:
      - traefik.enable=true
      - traefik.docker.network=proxy
      # Single router — the Rust binary handles every path, including
      # reverse-proxying HTML to its internal Next.js SSR upstream over
      # the compose bridge. WebSockets, OPDS, page bytes, HTML — all
      # one upstream.
      - traefik.http.routers.folio.rule=Host(`comics.example.com`)
      - traefik.http.routers.folio.entrypoints=websecure
      - traefik.http.routers.folio.tls.certresolver=le
      - traefik.http.services.folio.loadbalancer.server.port=8080
      # Streaming-friendly: page-bytes + OPDS PSE bodies must not be
      # buffered. Safe to apply globally since HTML responses are
      # small.
      - traefik.http.services.folio.loadbalancer.responseforwarding.flushInterval=100ms

  web:
    networks: [default]
    # `web` is internal-only on the compose bridge; the Rust binary
    # reaches it via `COMIC_WEB_UPSTREAM_URL=http://web:3000`. There
    # is no Traefik router for `web` — your external proxy never
    # talks to it directly.
```

Bring it up:

```bash
docker network create proxy 2>/dev/null || true
docker compose -f compose.prod.yml -f compose.traefik.yml up -d
```

## HSTS + X-Forwarded-*

Traefik passes `X-Forwarded-{For,Proto,Host}` by default when entry
points have `forwardedHeaders.insecure=true` for trusted client IPs.
Add HSTS via a middleware so the browser pins HTTPS:

```yaml
# In your traefik dynamic config:
http:
  middlewares:
    folio-hsts:
      headers:
        stsSeconds: 31536000
        stsIncludeSubdomains: true
        stsPreload: false   # set to true only after submitting to the preload list
```

Then attach it to the router via labels:

```yaml
- traefik.http.routers.folio.middlewares=folio-hsts@file
```

## Cookies

Folio uses `__Host-` / `__Secure-` cookies. Don't add any cookie-rewrite
middleware (`Set-Cookie` mutating, domain replacement, path rewriting) —
the prefixes require the original Path=/ and no Domain attribute, and
the cookies will silently drop if you mess with them.

## Verifying the wiring

```bash
# 1. Both routers should appear in the dashboard.
docker compose logs traefik | grep -i folio

# 2. App readiness through Traefik.
curl -fsS https://comics.example.com/readyz | jq

# 3. Web through Traefik.
curl -fsS https://comics.example.com/sign-in | grep -o '<title>.*</title>'

# 4. WebSocket upgrade.
wscat -c wss://comics.example.com/ws/scan -H "Cookie: __Host-comic_session=..."
```

## Common pitfalls

- **Letting Traefik buffer the OPDS Page-Streaming endpoint.** The
  `flushInterval=100ms` label above keeps the stream flowing — without
  it, OPDS readers see a stuttering progress bar.
- **Duplicating security headers.** Folio's Rust middleware already
  sets CSP, COOP, COEP, X-Frame-Options, X-Content-Type-Options,
  Referrer-Policy on every response (including HTML it reverse-proxies
  from Next). HSTS belongs at the proxy (above); the rest don't need
  to be duplicated.
- **Exposing `web` externally.** As of M5 of the rust-public-origin
  rollout, `web` is internal-only on the compose bridge. If you had a
  Traefik router pointed at `folio-web:3000` from a previous version,
  remove it — the Rust binary now reverse-proxies HTML internally.
