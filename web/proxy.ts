import createMiddleware from "next-intl/middleware";

import { DEFAULT_LOCALE, SUPPORTED_LOCALES } from "./i18n/request";

// Human-URLs M3: locale moves out of the URL into the `NEXT_LOCALE` cookie
// + per-user `language` preference. `localePrefix: "never"` tells next-intl
// to read the locale from the cookie / `Accept-Language` instead of the
// path segment. Routes live directly under `web/app/...`, no `[locale]/`.
//
// CSP nonce note: Next.js's App Router app-render layer auto-extracts
// the per-request nonce by parsing the `Content-Security-Policy`
// **request** header — the one the Rust origin sends on the proxy hop
// in `crates/server/src/upstream/mod.rs::forward()`. No middleware
// gymnastics needed here; setting the CSP request header is sufficient
// for Next to stamp `nonce="..."` onto every framework-emitted
// `<script>` tag. See
// `node_modules/next/dist/server/app-render/get-script-nonce-from-header.js`.
export default createMiddleware({
  locales: SUPPORTED_LOCALES as unknown as string[],
  defaultLocale: DEFAULT_LOCALE,
  localePrefix: "never",
});

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
