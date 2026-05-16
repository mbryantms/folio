import createMiddleware from "next-intl/middleware";

import { DEFAULT_LOCALE, SUPPORTED_LOCALES } from "./i18n/request";

// Human-URLs M3: locale moves out of the URL into the `NEXT_LOCALE` cookie
// + per-user `language` preference. `localePrefix: "never"` tells next-intl
// to read the locale from the cookie / `Accept-Language` instead of the
// path segment. Routes live directly under `web/app/...`, no `[locale]/`.
//
// Note on `?next=` redirect preservation: protected-layout guards
// (admin, library, settings) redirect to `/sign-in` without a
// `?next=` parameter today — Next 16's RSC layouts can't read the
// request URL without a documented proxy hack, and the proxy
// approaches we tried (header injection, response augmentation) all
// either lost next-intl's locale routing or didn't propagate to RSC.
// The sign-in page DOES honor an explicit `?next=` when the user
// (or a client-side caller like the OIDC start link) puts one on the
// URL. Layout-driven redirects currently land at `/` after auth.
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
