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
      # API + WebSocket + healthcheck + OPDS — anything served by the Rust binary.
      - traefik.http.routers.folio-app.rule=Host(`comics.example.com`) && (PathPrefix(`/api`) || PathPrefix(`/auth`) || PathPrefix(`/ws`) || PathPrefix(`/opds`) || PathPrefix(`/healthz`) || PathPrefix(`/readyz`) || PathPrefix(`/metrics`) || PathPrefix(`/csp-report`) || PathPrefix(`/issues`) || PathPrefix(`/pages`) || PathPrefix(`/thumbnails`))
      - traefik.http.routers.folio-app.entrypoints=websecure
      - traefik.http.routers.folio-app.tls.certresolver=le
      - traefik.http.routers.folio-app.priority=10
      - traefik.http.services.folio-app.loadbalancer.server.port=8080
      # Streaming-friendly: page-bytes + OPDS PSE must not be buffered.
      - traefik.http.services.folio-app.loadbalancer.responseforwarding.flushInterval=100ms

  web:
    networks: [default, proxy]
    ports: !reset []
    labels:
      - traefik.enable=true
      - traefik.docker.network=proxy
      # Catch-all: anything not claimed by the app router lands on Next.
      - traefik.http.routers.folio-web.rule=Host(`comics.example.com`)
      - traefik.http.routers.folio-web.entrypoints=websecure
      - traefik.http.routers.folio-web.tls.certresolver=le
      - traefik.http.routers.folio-web.priority=1
      - traefik.http.services.folio-web.loadbalancer.server.port=3000
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

Then attach it to both routers via labels:

```yaml
- traefik.http.routers.folio-app.middlewares=folio-hsts@file
- traefik.http.routers.folio-web.middlewares=folio-hsts@file
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

- **Forgetting `priority`**. Without it, the `folio-app` router (with
  the more specific PathPrefix) may lose to the catch-all `folio-web`.
  Set `app=10`, `web=1`.
- **Letting Traefik buffer the OPDS Page-Streaming endpoint.** The
  `flushInterval=100ms` label above keeps the stream flowing — without
  it, OPDS readers see a stuttering progress bar.
- **Duplicating security headers.** Folio's Rust middleware already
  sets CSP, COOP, COEP, X-Frame-Options, X-Content-Type-Options,
  Referrer-Policy. HSTS belongs at the proxy (above). The rest don't
  need to be duplicated.
