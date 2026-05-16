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
  if (nonce) {
    // Mutating `request.headers` directly is the documented Next 13+
    // pattern for surfacing values to RSC via `headers()`. next-intl
    // forwards the same headers through its internal rewrite, so the
    // value reaches both the locale router and the App-Router layer.
    request.headers.set("x-nonce", nonce);
  }
  // Delegate to next-intl for locale resolution + cookie-driven
  // rewrites. Returning its response unchanged preserves redirects
  // and rewrites — we only need our header injection to land before
  // next-intl runs.
  const response = intlMiddleware(request);
  return response ?? NextResponse.next();
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
