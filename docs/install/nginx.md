# nginx reverse proxy

Folio expects a TLS-terminating reverse proxy in front of the loopback
ports exposed by `compose.prod.yml`:

- `127.0.0.1:8080` → the Rust API + WebSocket + OPDS
- `127.0.0.1:3000` → the Next.js HTML / RSC server

You can use [Caddy](./caddy.md) (recommended for first-time
self-hosters — auto Let's Encrypt) or any other proxy. This page is the
reference for nginx.

## Required HTTP semantics

- **HTTP→HTTPS redirect** for all paths.
- **WebSocket upgrade** for `/ws*`. Without `Upgrade`/`Connection`
  headers, scan-progress events won't reach the admin UI.
- **No response buffering** for OPDS page-streaming and page-bytes —
  buffering breaks chunked image streams.
- **`X-Forwarded-{For,Proto,Host}`** so the Rust server's rate
  limiter sees the real client IP. The app trusts `COMIC_TRUSTED_PROXIES`
  for this header; the compose default-bridge range `172.16.0.0/12`
  covers the case where nginx runs on the docker host.
- **Cookies untouched.** Folio uses `__Host-` / `__Secure-` prefixed
  cookies which require Secure + the original Path. Don't rewrite cookie
  domain or path.

## Why the Rust server ALSO sets headers

[`security_headers`][sh] sets CSP, COOP, COEP, X-Content-Type-Options,
X-Frame-Options, and Referrer-Policy on every response. Setting them in
nginx as well is redundant; matching values are fine, mismatched ones
will be confusing during debugging. **HSTS is the one header you DO
want nginx to set** — the app doesn't know whether the cert is real, so
it's the proxy's call.

[sh]: ../../crates/server/src/middleware/security_headers.rs

## Example vhost

```nginx
# /etc/nginx/sites-available/folio
#
# Single upstream — the Rust binary is the public origin. It handles
# its own routes (/api, /opds, /auth, page bytes, /ws/*) directly and
# reverse-proxies HTML + /_next/* assets to its internal Next.js SSR
# upstream over the compose bridge. nginx does not talk to `web`
# directly.
upstream folio_app { server 127.0.0.1:8080; keepalive 16; }

# HTTP → HTTPS redirect (Let's Encrypt HTTP-01 challenge handled by certbot).
server {
    listen 80;
    listen [::]:80;
    server_name comics.example.com;
    location /.well-known/acme-challenge/ { root /var/www/letsencrypt; }
    location / { return 308 https://$host$request_uri; }
}

server {
    listen 443 ssl http2;
    listen [::]:443 ssl http2;
    server_name comics.example.com;

    ssl_certificate     /etc/letsencrypt/live/comics.example.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/comics.example.com/privkey.pem;
    ssl_session_cache shared:SSL:10m;
    ssl_session_timeout 10m;
    ssl_protocols TLSv1.2 TLSv1.3;

    # HSTS — see "Why the Rust server ALSO sets headers" above.
    add_header Strict-Transport-Security "max-age=31536000; includeSubDomains" always;

    # Generous body limit so the admin "bulk import" CBL endpoint doesn't 413.
    client_max_body_size 64m;

    # Standard X-Forwarded-* set for every location.
    proxy_set_header Host              $host;
    proxy_set_header X-Real-IP         $remote_addr;
    proxy_set_header X-Forwarded-For   $proxy_add_x_forwarded_for;
    proxy_set_header X-Forwarded-Proto $scheme;
    proxy_set_header X-Forwarded-Host  $host;
    proxy_http_version 1.1;

    # WebSocket upgrade headers — apply to every upstream conn since
    # `/ws/*` and the Next SSR proxy both need them. nginx leaves them
    # alone for non-upgrade requests.
    proxy_set_header Upgrade    $http_upgrade;
    proxy_set_header Connection $connection_upgrade;
    proxy_read_timeout 600s;
    proxy_send_timeout 600s;

    # No response buffering — Folio streams page bytes + OPDS PSE
    # bodies, and buffering would break Range request UX and bloat
    # nginx's RSS on large reads. Safe everywhere because HTML
    # responses are small enough that streaming costs nothing.
    proxy_buffering off;
    proxy_request_buffering off;

    location / {
        proxy_pass http://folio_app;
    }
}

# nginx needs this map for the WebSocket Connection header to flip
# correctly between `upgrade` (when Upgrade is set) and empty (when
# it's a normal request). Drop it in `/etc/nginx/conf.d/`.
#
#   map $http_upgrade $connection_upgrade {
#       default upgrade;
#       ''      close;
#   }
```

## Verifying the wiring

```bash
# 1. HTTP → HTTPS redirect.
curl -sI http://comics.example.com/ | grep -i location

# 2. App readiness through the proxy.
curl -fsS https://comics.example.com/readyz | jq

# 3. Web frontend through the proxy.
curl -fsS https://comics.example.com/sign-in | grep -o '<title>.*</title>'

# 4. WebSocket upgrade (requires `wscat` or similar).
wscat -c wss://comics.example.com/ws/scan -H "Cookie: __Host-comic_session=..."
```

## Common pitfalls

- **Hot-rewriting cookies.** Don't `proxy_cookie_path` or `proxy_cookie_domain`.
  Folio uses `__Host-` cookies, which require the original Path=/ and no Domain.
- **Stripping headers**. nginx defaults silently drop hop-by-hop headers; the
  list above covers the ones the app needs. If you're using a custom config,
  ensure `X-Forwarded-For` reaches the app — without it the rate limiter
  treats every request as same-origin and fails open.
- **`proxy_buffering on` for OPDS-PSE.** Some readers (Chunky, Panels) issue
  range requests against a streaming endpoint. nginx-buffered responses break
  this with a stuttering reader UI.
- **No HSTS preload header**. Don't add `; preload` unless you've actually
  submitted to the preload list. Reverting later requires waiting out the
  max-age.
