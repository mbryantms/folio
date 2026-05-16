import createIntlMiddleware from "next-intl/middleware";
import { NextResponse, type NextRequest } from "next/server";

import { DEFAULT_LOCALE, SUPPORTED_LOCALES } from "./i18n/request";

// Human-URLs M3: locale moves out of the URL into the `NEXT_LOCALE` cookie
// + per-user `language` preference. `localePrefix: "never"` tells next-intl
// to read the locale from the cookie / `Accept-Language` instead of the
// path segment. Routes live directly under `web/app/...`, no `[locale]/`.
//
// CSP nonce (csp-nonce-1.0 plan, M4): the Rust origin generates a
// per-request nonce in `crates/server/src/middleware/nonce.rs` and
// forwards it to us via `x-csp-nonce`. We republish it as `x-nonce`
// on the inbound request so Next.js auto-stamps the value onto every
// framework-emitted `<script>` tag in the SSR HTML — see Next's
// [CSP guide](https://nextjs.org/docs/app/guides/content-security-policy).
// RSC server components that need to nonce their own `<Script>` can
// read it via `(await headers()).get('x-nonce')`.
const intlMiddleware = createIntlMiddleware({
  locales: SUPPORTED_LOCALES as unknown as string[],
  defaultLocale: DEFAULT_LOCALE,
  localePrefix: "never",
});

export default function middleware(request: NextRequest) {
  const nonce = request.headers.get("x-csp-nonce");
  // Defensively mutate `request.headers` so anything next-intl reads
  // inside its middleware sees the same value (it doesn't today, but
  // keeps the two views consistent).
  if (nonce) {
    request.headers.set("x-nonce", nonce);
  }
  // Delegate to next-intl for locale resolution + cookie-driven
  // rewrites. It returns a NextResponse that may be a rewrite (locale
  // routing), redirect, or pass-through.
  const response = intlMiddleware(request) ?? NextResponse.next();
  // Tell Next.js's internal router to forward `x-nonce` to the
  // page/RSC handler. This is the underlying mechanism that
  // `NextResponse.next({ request: { headers } })` uses; we apply it
  // by hand because we need to stack the override on top of
  // next-intl's response (which we can't replace without losing its
  // rewrite/redirect). Without this, mutating `request.headers`
  // above does NOT propagate past next-intl's rewrite — RSC's
  // `headers()` call sees the original inbound headers and Next.js
  // doesn't auto-stamp the nonce onto framework-emitted `<script>`
  // tags, leaving `'strict-dynamic'` to block everything.
  if (nonce) {
    const existing = response.headers.get("x-middleware-override-headers");
    const overrides = existing ? `${existing},x-nonce` : "x-nonce";
    response.headers.set("x-middleware-override-headers", overrides);
    response.headers.set("x-middleware-request-x-nonce", nonce);
  }
  return response;
}

export const config = {
  // With `localePrefix: "never"`, next-intl internally rewrites every
  // matched request to `/{locale}/{path}` so the file-system router can
  // resolve `app/[locale]/...` pages. We only need to exclude paths
  // that aren't HTML routes Next owns:
  //
  //   - `/_next/*` — Next's own static-asset namespace; never rewritten.
  //   - `*.ext`   — files; HTML routing doesn't apply.
  //
  // As of v0.2 (rust-public-origin), the Rust binary is the public
  // origin in prod and reverse-proxies HTML/RSC requests to Next via
  // an internal upstream. Backend paths the Rust router owns
  // (`/api/*`, `/opds/*`, `/auth/*`, `/issues/*`, etc.) never reach
  // Next.js at all — there is no need (and no reason) to list them
  // here. The earlier per-prefix exclusions for `opds`, `auth/oidc`,
  // and `issues` were workarounds for the previous Next-as-front
  // topology and have been removed. Do not re-add them.
  matcher: ["/((?!_next|.*\\..*).*)"],
};
