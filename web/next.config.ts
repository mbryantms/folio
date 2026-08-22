import type { NextConfig } from "next";
import createNextIntlPlugin from "next-intl/plugin";

const withNextIntl = createNextIntlPlugin("./i18n/request.ts");

// Server Actions are stable in Next 15 (no boolean disable available); we keep
// our single auth path by simply not using them (§15.7, §17.3). Edge runtime
// is not used (§C3) — all routes run in Node.
const config: NextConfig = {
  output: "standalone",
  // next 16.3.1 bumped @swc/helpers 0.5.15 -> 0.5.23, which added a
  // `module-sync` export condition:
  //   0.5.15  { import: esm/…, default: cjs/….cjs }
  //   0.5.23  { module-sync: esm/…, webpack: esm/…, import: …, default: … }
  // Next's `require-hook.js` pulls the helpers in with `require()`. The
  // standalone file tracer resolves that through `default` and copies only
  // `cjs/`, but Node >=22.10 honours `module-sync` FIRST and asks for
  // `esm/` — which was never copied. The container then dies at boot with
  //   MODULE_NOT_FOUND …/@swc/helpers/esm/_interop_require_default.js
  // and restart-loops (shipped in v0.27.5).
  //
  // `next build` cannot catch this: the tracer does not validate its own
  // output, so the build is green and the bundle is silently incomplete.
  // Only booting the image fails — hence the compose smoke test now gating
  // the image build in CI. Force the whole package in until the tracer
  // learns the condition; this is additive, so it cannot change resolution.
  outputFileTracingIncludes: {
    "**/*": [
      "../node_modules/.pnpm/@swc+helpers@*/node_modules/@swc/helpers/**",
    ],
  },
  reactStrictMode: true,
  poweredByHeader: false,
  // React Compiler (audit G4, chunk 1.0b). Auto-memoizes components and
  // hooks at build time so render hygiene stops regressing per-site —
  // Wave-0 chunk 0.3 added the hottest React.memo wrappers by hand; the
  // compiler makes the rest free. Babel-based (babel-plugin-react-compiler),
  // applied on both the webpack dev path and the Turbopack prod build.
  reactCompiler: true,
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

// Service-worker compilation is intentionally NOT wired into the Next
// build via `@serwist/next`. That plugin requires Webpack, which
// produces a chunk-graph topology ~50% heavier than Turbopack's for the
// same source — large enough to blow the §18.1 reader-bundle budget
// (see `web/scripts/check-bundle-size.mjs`). Instead, the SW is
// compiled in a separate post-`next build` step via `@serwist/cli`
// (see `serwist.config.js` and the `sw:compile` package script). The
// SW source still lives at `app/sw.ts` and the output still lands at
// `public/sw.js`; only the compile vehicle changed.
export default withNextIntl(config);
