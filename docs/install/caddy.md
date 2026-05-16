# Caddy reverse proxy

Folio runs as a single public-facing service behind a TLS-terminating
proxy (`compose.prod.yml`):

- `app:8080` — Rust binary. Owns `/api/*`, `/opds/*`, `/auth/*`,
  page bytes, WebSockets, _and_ HTML pages. It reverse-proxies
  HTML/RSC/`/_next/*` requests to the Next.js SSR upstream (`web:3000`)
  internally over the compose bridge — your proxy never talks to
  `web` directly.

Caddy is the recommended first-time setup because it handles Let's
Encrypt automatically and HTTP/3 is free. Drop the Caddyfile below into
`/etc/caddy/Caddyfile`, set `comics.example.com` + `admin@example.com`,
restart Caddy.

> **Security headers — don't duplicate.** The Rust server's
> `security_headers` middleware already sets `Content-Security-Policy`,
> `Cross-Origin-{Opener,Embedder,Resource}-Policy`, `X-Frame-Options`,
> `X-Content-Type-Options`, `Referrer-Policy`, and `Permissions-Policy`
> on every response (including HTML pages it proxies from Next). The
> Caddyfile below only adds **HSTS**, which the app can't set (it
> doesn't know whether the cert is real). Don't add a
> `Content-Security-Policy` line in Caddy — you'd override the
> nonce-bearing one the app emits.

```caddyfile
# /etc/caddy/Caddyfile
{
    email admin@example.com
    servers {
        protocols h1 h2 h3
    }
}

comics.example.com {
    encode zstd gzip

    # HSTS is the proxy's job; the rest of the security-header surface is
    # owned by the Rust middleware (see callout above).
    header {
        Strict-Transport-Security "max-age=63072000; includeSubDomains"
        -Server
    }

    # Single upstream — Rust handles every path, proxying HTML to its
    # internal Next.js upstream as needed. WebSockets, streamed OPDS/PSE
    # bodies, page bytes, and HTML are all served through this one
    # `reverse_proxy` directive.
    reverse_proxy app:8080 {
        # Disable response buffering. Folio streams page bytes + OPDS
        # PSE bodies; buffering would break Range request UX and bloat
        # Caddy's RSS on large reads.
        flush_interval -1
        transport http {
            # Long-lived for WebSocket upgrades (scan progress, presence).
            read_timeout  10m
            write_timeout 10m
            keepalive 60s
        }
    }
}
```

## Compose docker network

For Caddy to reach `app:8080` by name, Caddy and the Folio services
need to share a docker network. The simplest layout is to add Caddy as
a service to your existing compose project; it joins the default
network automatically and can resolve sibling services.

If you prefer to run Caddy on the host instead of in compose, change
the `reverse_proxy app:8080` line to `reverse_proxy 127.0.0.1:8080` —
the default `compose.prod.yml` binds `app` to loopback. (`web` is no
longer published to the host at all; its port lives only on the
compose bridge.)

## Required `COMIC_TRUSTED_PROXIES`

Set it to the bridge subnet Caddy lives on (or its specific container
IP) so the Rust server trusts `X-Forwarded-For` from Caddy:

```env
COMIC_TRUSTED_PROXIES=172.18.0.0/16
```

`compose.prod.yml` defaults this to `172.16.0.0/12` (the docker default
bridge range). Without this header the rate limiter treats every
request as coming from Caddy and fails open per-IP.

## Verifying the wiring

```bash
# 1. App readiness through Caddy.
curl -fsS https://comics.example.com/readyz | jq

# 2. Web frontend through Caddy.
curl -fsS https://comics.example.com/sign-in | grep -o '<title>.*</title>'

# 3. HSTS header is present.
curl -sI https://comics.example.com/ | grep -i strict-transport-security

# 4. CSP came from the app, not Caddy (look for the nonce).
curl -sI https://comics.example.com/sign-in | grep -i content-security-policy
```

## Notes

- `flush_interval -1` on the OPDS-PSE + page-byte routes is critical;
  without it Caddy buffers the response in memory, breaking Range
  request UX and inflating RSS on large pages.
- `comics.example.com` triggers automatic Let's Encrypt issuance. For
  air-gapped or self-signed deployments without a public domain, see
  [`lan-https-mkcert.md`](./lan-https-mkcert.md).
- Caddy auto-enables HTTP/3 once HTTPS is in place; no application
  change required.
- For nginx / Traefik equivalents see [`nginx.md`](./nginx.md) and
  [`traefik.md`](./traefik.md).
