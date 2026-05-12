# CSP & Security Headers

Phase 0 deliverable. Implementation in [crates/server/src/middleware/security_headers.rs](../../crates/server/src/middleware/security_headers.rs). Source of truth: [comic-reader-spec.md §17.4](../../comic-reader-spec.md).

## Policy

```
Content-Security-Policy:
  default-src 'self';
  script-src 'self' 'strict-dynamic';
  style-src 'self';
  img-src 'self' data: blob:;
  font-src 'self';
  connect-src 'self' {OIDC_ISSUER_ORIGIN} wss://{HOST};
  frame-ancestors 'none';
  form-action 'self';
  base-uri 'none';
  object-src 'none';
  worker-src 'self' blob:;
  manifest-src 'self';
  require-trusted-types-for 'script';
  upgrade-insecure-requests;
  report-to comic-csp;
```

`{OIDC_ISSUER_ORIGIN}` and `{HOST}` are computed at startup from `COMIC_OIDC_ISSUER` and `COMIC_PUBLIC_URL` respectively. The Rust middleware emits this for **every** response (JSON, bytes, HTML).

For HTML pages served by Next.js, a per-request **nonce** is injected by Next.js middleware so inline `<script nonce="…">` and `<style nonce="…">` work without `'unsafe-inline'`. The Rust layer's CSP uses `'strict-dynamic'` so a nonced loader can `import` further scripts.

## Companion headers

| Header | Value |
|---|---|
| `Strict-Transport-Security` | `max-age=63072000; includeSubDomains` (when public URL is HTTPS). |
| `Cross-Origin-Opener-Policy` | `same-origin` |
| `Cross-Origin-Embedder-Policy` | `credentialless` (enables SharedArrayBuffer in worker decode) |
| `Cross-Origin-Resource-Policy` | `same-origin` |
| `Referrer-Policy` | `strict-origin-when-cross-origin` |
| `Permissions-Policy` | `camera=(), microphone=(), geolocation=(), usb=(), bluetooth=(), payment=()` |
| `X-Content-Type-Options` | `nosniff` |
| `X-Frame-Options` | `DENY` (legacy belt-and-braces; `frame-ancestors 'none'` is the load-bearing rule) |

## Reporting

CSP violation reports POST to `/csp-report`. Modern browsers send `application/reports+json` envelopes; legacy send `application/csp-report`. The handler accepts both, increments the `comic_csp_violations_total` counter, and logs at `warn` with the full report body.

Rate-limited (§17.7): 100/min/IP. A misbehaving extension can otherwise flood the endpoint.

## Trusted Types

`require-trusted-types-for 'script'` forces all DOM sinks (`innerHTML`, `Element.setAttribute('on…')`, …) to require a Trusted Type. The Next.js layer provides a single `TrustedTypePolicy` wrapping `DOMPurify` for the Tiptap review editor (Phase 5 only); other code never produces strings that hit script sinks.

## Verification

Integration test (`crates/server/tests/security_headers.rs`) hits `/`, `/healthz`, `/openapi.json`, `/csp-report` and asserts every header listed above is present and matches the spec exactly. Failure = CI fail.

Manual verification:

```bash
curl -sI http://localhost:8080/healthz | grep -iE 'content-security|cross-origin|referrer|permissions|x-(content|frame)|strict-transport'
```

Should print all eight headers.
