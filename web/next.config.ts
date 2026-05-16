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
  // As of v0.2 (rust-public-origin plan, M4 follow-up), the Rust binary
  // is the public origin and reverse-proxies HTML/RSC/`/_next/*` here.
  // The web app fetches backend paths directly (`fetch("/series/...")`)
  // — there is no Next-side `/api/*` rewrite alias any more. Security
  // headers (CSP, COOP, COEP, etc.) are set by the Rust
  // `security_headers` middleware on every response, including HTML
  // proxied back from Next.
  //
  // DO NOT add rewrites here for backend paths. With Rust as the
  // public origin, every path the Rust router owns (or that its
  // fallback proxy forwards back to here) is reachable directly. The
  // v0.1.15-17 rewrites for `/opds/*`, `/auth/oidc/*`, `/issues/*`,
  // and the v0.2-transient `/api/:path*` alias are all gone — they
  // were workarounds for the old Next-as-front topology and no longer
  // apply.
};

export default withNextIntl(config);
