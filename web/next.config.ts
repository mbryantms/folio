import type { NextConfig } from "next";
import createNextIntlPlugin from "next-intl/plugin";

const withNextIntl = createNextIntlPlugin("./i18n/request.ts");

// Server Actions are stable in Next 15 (no boolean disable available); we keep
// our single auth path by simply not using them (§15.7, §17.3). Edge runtime
// is not used (§C3) — all routes run in Node.
const config: NextConfig = {
  output: "standalone",
  reactStrictMode: true,
  poweredByHeader: false,
  // Allow LAN-origin requests at dev. Next 16 dev's asset cross-origin
  // checks reject anything that isn't a localhost variant by default; without
  // this list, browsing to e.g. http://192.168.1.x:3000 succeeds for SSR but
  // hydration silently fails — leaving the page interactive only for
  // browser-native form submission (which is GET by default, and is exactly
  // what leaked `?email=&password=` into the URL in M9). The wildcard hosts
  // here cover RFC-1918 ranges that user home routers typically hand out.
  allowedDevOrigins: [
    "192.168.0.0/16",
    "10.0.0.0/8",
    "172.16.0.0/12",
    "*.local",
  ],
  // The Rust server reverse-proxies us, so we don't bind directly to a public host.
  // Headers (CSP, COOP, COEP, etc.) are set by the Rust security_headers middleware
  // on every response, including HTML proxied through from Next.
  async rewrites() {
    // `API_PROXY_URL` is read at *build* time — Next freezes the rewrites
    // array into `.next/routes-manifest.json` and the runtime server never
    // re-evaluates it. The prod Dockerfile bakes the compose-internal
    // value `http://app:8080` via an ARG; dev (`pnpm dev`) falls through
    // to localhost. Setting it via runtime env on a published image has no
    // effect — rebuild the image to change it. Intentionally NOT prefixed
    // with `NEXT_PUBLIC_`: this hostname is server-only and must not be
    // inlined into the client bundle.
    const apiBase = process.env.API_PROXY_URL || "http://localhost:8080";
    return [
      // Proxy API calls to the Rust server. The `/api/` prefix is a
      // Next-only namespace and is stripped before hitting the backend.
      // NB: Next dev's rewrite layer does NOT support WebSocket upgrades —
      // `/ws/*` cannot be proxied here. The WS client connects to the Rust
      // server directly; auth lands via the §9.6 ticket flow (carry-over).
      {
        source: "/api/:path*",
        destination: `${apiBase}/:path*`,
      },
      // Externally-addressable backend paths that *cannot* be `/api/`-
      // prefixed: OPDS clients (Panels, KOReader, Chunky, etc.) hit
      // `/opds/v1` directly, and OIDC IdPs redirect back to
      // `/auth/oidc/callback`. Without these rewrites, when Next.js is
      // the public origin those paths hit the i18n middleware, get
      // rewritten to `/en/opds/...`, find no page, and 404 with HTML —
      // which third-party clients see as "invalid server response".
      // Harmless when the Rust binary is the public origin instead
      // (the rewrite never fires because the request never reaches
      // Next in the first place).
      {
        source: "/opds/:path*",
        destination: `${apiBase}/opds/:path*`,
      },
      {
        source: "/auth/oidc/:path*",
        destination: `${apiBase}/auth/oidc/:path*`,
      },
    ];
  },
};

export default withNextIntl(config);
