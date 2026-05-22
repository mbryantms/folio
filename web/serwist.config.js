/**
 * `@serwist/cli build` configuration.
 *
 * Why standalone (vs `@serwist/next` as a plugin):
 *
 * @serwist/next 9.x is a Webpack plugin and refuses to run under
 * Turbopack (serwist/serwist#54). Pinning the Next build to
 * `--webpack` for the SW compile was rejected because Webpack's
 * chunk-graph topology roughly doubled the reader-bundle measurement
 * the §18.1 budget gate is supposed to police (158 KB / 15 chunks
 * under Turbopack → 224 KB / 38 chunks under Webpack, for the same
 * source).
 *
 * Splitting the SW compile into its own post-`next build` step keeps
 * the Next build on Turbopack (where it's fastest and bundles
 * tightest) and uses @serwist/cli — same upstream codebase as the
 * Next plugin — to bundle `app/sw.ts` + inject Next's static-asset
 * precache manifest. The two-step `build` script in package.json
 * chains them.
 *
 * What gets precached:
 *
 * - The compiled JS chunks Next emits under `.next/static/chunks/`.
 *   These rarely change between deploys for unchanged code paths
 *   (content-hashed filenames), and they're what makes the app shell
 *   launch offline.
 * - Stylesheets under `.next/static/css/`.
 *
 * What does NOT get precached (intentional):
 *
 * - Images / fonts / media. These are heavy and the browser caches
 *   them well on its own via long-immutable Cache-Control headers
 *   from the Rust origin (§17.5).
 * - Page bytes (`/issues/{id}/pages/{n}`). Per-user-authorised, far
 *   too large to ship in a precache, and the offline-reading feature
 *   that would intentionally cache them gets its own driver (see
 *   PWA hardening Tier 2 follow-up notes).
 * - HTML routes. Server-rendered, change per-deploy, never matched
 *   by the static-chunk globs.
 *
 * URLs are rebased to the public `/_next/static/...` paths the user
 * actually fetches via `manifestTransforms` so the precache entries
 * line up with real fetches.
 */
const config = {
  swSrc: "app/sw.ts",
  swDest: "public/sw.js",
  // `self.__SW_MANIFEST` is the placeholder `app/sw.ts` reads via
  // the type declaration on `ServiceWorkerGlobalScope`. @serwist/cli
  // replaces this exact identifier with the materialised manifest
  // at compile time. The default value matches @serwist/next so the
  // SW source compiles identically either way.
  injectionPoint: "self.__SW_MANIFEST",
  globDirectory: ".next/static",
  globPatterns: ["**/*.{js,css}"],
  // Skip whatever bytes are too volatile to be worth precaching:
  // - service worker output itself (in the unlikely event the
  //   globber wanders into public/)
  // - sourcemaps
  globIgnores: ["**/*.map", "**/sw.js", "**/swe-worker-*.js"],
  // Next stamps content hashes into the chunk filenames already, so
  // we don't need revisions on top of that.
  dontCacheBustURLsMatching: /\/_next\/static\//,
  // The CLI globs files relative to `globDirectory`, producing URLs
  // like `chunks/abc.js`. The runtime fetches them at
  // `/_next/static/chunks/abc.js`. Rebase every manifest entry so
  // the URLs line up with the live HTTP paths.
  manifestTransforms: [
    async (entries) => ({
      manifest: entries.map((entry) => ({
        ...entry,
        url: `/_next/static/${entry.url}`,
      })),
    }),
  ],
};

export default config;
