# Cloudflare in front of Folio

Folio is happy to sit behind Cloudflare — TLS termination, edge caching
for static `/_next/*` chunks, and WAF all compose cleanly. There's one
**required setting change**, a choice between two supported ways of
connecting Cloudflare to your origin, and a couple of recommended
settings.

## Required: disable Email Address Obfuscation

Path: **Cloudflare dashboard → your zone → Scrape Shield → Email Address Obfuscation → off**.

Cloudflare's email-obfuscation feature injects an inline `<script>`
tag from `/cdn-cgi/scripts/.../email-decode.min.js` into every HTML
response that contains an email address. The script ends up in pages
like `/admin/users`, `/settings/account`, password-reset confirmation
screens, and any other place we render a user's email. The injected
script tag carries no nonce, and Folio's CSP only runs scripts that do:
`script-src 'nonce-…' 'strict-dynamic'`, with no `'unsafe-inline'` and
no host allowlist, so the browser blocks it. (Trusted Types are not
enforced today; the CSP has no `require-trusted-types-for` directive
because the React runtime still writes `innerHTML` directly.)

You'll see the block in the browser console as repeated
`Content-Security-Policy: blocked … from 'https://<host>/cdn-cgi/scripts/…/email-decode.min.js'`.
Disabling Email Address Obfuscation removes the injection and the
errors go away. The feature is designed for static HTML sites that
don't already proxy email-sensitive data through their own backend;
Folio's account pages already gate email visibility on auth, so
there's nothing to obfuscate at the edge.

## Connecting Cloudflare to the origin

Cloudflare must never reach Folio over plaintext HTTP from the public
internet. Pointing an origin rule at `http://<origin-ip>:8080` means the
app port is open to the world, the hop between Cloudflare and your host
is unencrypted, and anyone who finds the address can skip Cloudflare
entirely. Pick one of the two shapes below instead.

Either way the rule on the Cloudflare side is a single catch-all for
the hostname. Since v0.2.0 the Rust binary on `app:8080` is the only
public-facing service: it serves API routes and WebSocket upgrades
itself and proxies HTML to Next.js over the compose bridge. If you
still have an older rule splitting `/ws/*` to `:8080` and `*` to
`:3000`, delete it (see [`upgrades.md`](./upgrades.md#v020--rust-binary-becomes-the-public-origin)).

### Option A: Cloudflare Tunnel

Run `cloudflared` as a container in the same compose project, so it
shares the compose network with `app`. The tunnel dials out to
Cloudflare; nothing on the host listens on a public port.

```yaml
# compose.cloudflared.yml — layer onto compose.prod.yml:
#   docker compose -f compose.prod.yml -f compose.cloudflared.yml up -d
services:
  cloudflared:
    image: cloudflare/cloudflared:latest
    restart: unless-stopped
    command: tunnel --no-autoupdate run
    environment:
      TUNNEL_TOKEN: ${CLOUDFLARE_TUNNEL_TOKEN:?required}
    depends_on:
      - app
```

In the Zero Trust dashboard, give the tunnel a public hostname
(`comics.example.com`) whose service is `http://app:8080`. The hop from
`cloudflared` to `app` stays on the compose bridge; the tunnel itself is
encrypted. Pin the `cloudflared` image to a version or digest once it
works, the same way `compose.prod.yml` pins its own images.

Leave `COMIC_APP_BIND` unset so `app` stays bound to `127.0.0.1`. The
tunnel does not need the host port at all; loopback keeps it available
for `curl` checks on the host and nothing else.

### Option B: Cloudflare in front of your own reverse proxy

Keep the proxy you would run without Cloudflare — the
[Caddy](./caddy.md), [nginx](./nginx.md) or [Traefik](./traefik.md)
template — and point Cloudflare's DNS record at it, proxied (orange
cloud). Then:

- Set **SSL/TLS → Overview → encryption mode** to **Full (strict)**. The
  proxy needs a certificate Cloudflare will validate: a Let's Encrypt
  certificate (Caddy gets one automatically) or a Cloudflare Origin CA
  certificate. Flexible and plain Full are not supported: Flexible sends
  plaintext to your origin, and Full accepts any certificate.
- Firewall ports 80 and 443 on the origin so they accept connections
  only from Cloudflare's published address ranges
  (<https://www.cloudflare.com/ips/>), which change occasionally.
  Otherwise anyone can reach the proxy directly and bypass the WAF.
- Keep `app` on its default loopback bind; only the proxy talks to it.

## Client addresses: `COMIC_TRUSTED_PROXIES`

Folio's per-address rate limits, the failed sign-in lockout, the audit
log and the **Active sessions** list under **Settings → Account** all
use the client address Folio resolves for each request. Folio reads only
`X-Forwarded-For`. It does **not** read `CF-Connecting-IP` or
`X-Real-IP`.

The rule
([`auth/xff.rs`](../../crates/server/src/auth/xff.rs)): if the TCP peer
that connected to `app` is not inside `COMIC_TRUSTED_PROXIES`, Folio
uses the peer address and ignores the header. If the peer is trusted,
Folio walks `X-Forwarded-For` from the right and takes the first
address that is not inside `COMIC_TRUSTED_PROXIES`. The variable is a
comma-separated list of CIDRs or bare IPs; `compose.prod.yml` defaults
it to `172.16.0.0/12`, which covers Docker's default bridge networks.
(If a request carries a `Forwarded:` header with `for=`, Folio falls
back to the peer address, so don't add one in your proxy.)

Cloudflare's edge puts the visitor's address in `X-Forwarded-For`, so
what has to be trusted depends on the shape:

- **Option A (tunnel).** `cloudflared` passes Cloudflare's
  `X-Forwarded-For` through unchanged and is itself the peer. Trust the
  compose network `cloudflared` runs on; the default `172.16.0.0/12`
  already does. Don't add Cloudflare's ranges — no Cloudflare address
  appears in the chain.
- **Option B (Cloudflare → proxy → app).** The proxy appends the
  Cloudflare edge address it received the request from, so the chain
  reaching Folio ends in a Cloudflare address. Trust both the proxy's
  network and every Cloudflare range:

  ```env
  COMIC_TRUSTED_PROXIES=172.16.0.0/12,173.245.48.0/20,103.21.244.0/22,…,2400:cb00::/32,…
  ```

  Copy the full IPv4 and IPv6 lists from
  <https://www.cloudflare.com/ips/> rather than the sample above, and
  refresh them when Cloudflare changes them. Without the Cloudflare
  ranges every visitor appears as a Cloudflare edge server, and the
  per-address limits group unrelated users together.

  The proxy must also keep Cloudflare's `X-Forwarded-For` and append to
  it rather than replace it. nginx's `$proxy_add_x_forwarded_for` does
  this as-is. Caddy and Traefik discard incoming forwarding headers from
  peers they don't trust, so list Cloudflare's ranges there too: Caddy's
  global `servers { trusted_proxies static … }` option, Traefik's
  `entryPoints.<name>.forwardedHeaders.trustedIPs`. If the proxy
  replaces the header, Folio sees only the edge address.

To check the result, sign in through Cloudflare and open **Settings →
Account → Active sessions**: the address shown should be yours, not a
Cloudflare or Docker address.

## Recommended: cache `/_next/static/*` aggressively

Next.js builds `/_next/static/chunks/<hash>.js` filenames with the
content hash, so they're safe to cache forever at the edge:

```
URL match: ${host}/_next/static/*
Edge cache TTL: 1 year
Browser cache TTL: respect existing headers
```

The Rust binary already proxies these from Next's standalone bundle
with the right `Cache-Control: public, max-age=31536000, immutable`
headers, so this is purely an edge-cache opt-in.

## Optional: don't cache HTML or `/api/*`

Defaults are usually fine — Cloudflare won't cache anything not in its
static extension list out of the box — but if you want explicit page
rules:

```
${host}/api/*           → Cache Level: Bypass
${host}/opds/*          → Cache Level: Bypass
${host}/auth/*          → Cache Level: Bypass
${host}/                → Cache Level: Bypass
```

These all carry session-scoped data; caching them risks leaking one
user's view to another.
