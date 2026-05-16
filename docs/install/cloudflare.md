# Cloudflare in front of Folio

Folio is happy to sit behind Cloudflare — TLS termination, edge caching
for static `/_next/*` chunks, and WAF all compose cleanly. There's one
**required setting change** plus a couple of recommended ones.

## Required: disable Email Address Obfuscation

Path: **Cloudflare dashboard → your zone → Scrape Shield → Email Address Obfuscation → off**.

Cloudflare's email-obfuscation feature injects an inline `<script>`
tag from `/cdn-cgi/scripts/.../email-decode.min.js` into every HTML
response that contains an email address. The script ends up in pages
like `/admin/users`, `/settings/account`, password-reset confirmation
screens, and any other place we render a user's email. Two things
about this script break Folio's CSP:

- The injected script tag has no nonce, so `script-src 'self' 'nonce-…' 'strict-dynamic'`
  blocks it.
- The script writes through `Element.innerHTML`, which violates
  `require-trusted-types-for 'script'`.

You'll see both errors in the browser console as repeated
`Content-Security-Policy: blocked … from 'https://<host>/cdn-cgi/scripts/…/email-decode.min.js'`.
Disabling Email Address Obfuscation removes the injection and the
errors go away. The feature is designed for static HTML sites that
don't already proxy email-sensitive data through their own backend;
Folio's account pages already gate email visibility on auth, so
there's nothing to obfuscate at the edge.

## Recommended: single rule, port 8080

If you're using Cloudflare's "Origin Rules" (or page rules) to map
`folio.example.com` to your homelab origin, the rule should be a
single catch-all pointing at the Rust port:

```
* → http://<origin-ip>:8080
```

If you have an older rule from before v0.2 splitting `/ws/*` to `:8080`
and `*` to `:3000` (Next.js), delete the `/ws/*` rule and update the
catch-all to `:8080`. As of v0.2.0 the Rust binary is the single
public-facing service — it serves API routes directly, WebSocket
upgrades natively, and reverse-proxies HTML to Next.js over the
compose bridge.

See [`docs/install/upgrades.md#v020`](./upgrades.md) for the broader
breaking-change summary.

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
