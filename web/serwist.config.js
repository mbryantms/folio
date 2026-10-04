import { readFileSync } from "node:fs";

/** Compile separately from Next so production can keep using Turbopack.
 * Only the public offline document is precached. Hashed Next assets are
 * cached on demand; visiting the library does not download admin bundles.
 *
 * Every release must produce a byte-different `sw.js`: the browser only
 * installs (and `ServiceWorkerUpdater` only offers "Reload") when the
 * worker script changes. With `offline.html` as the sole precache entry,
 * a release that touches neither it nor `app/sw.ts` / `lib/pwa/*` shipped
 * an identical worker, so open tabs and installed apps were never told
 * about it (v0.29.1 – v0.34.0). Folding Next's build id into the entry's
 * revision changes the script on every build; the cost is one re-fetch
 * of the ~1 KB offline page per release.
 */
export function buildId(path = ".next/BUILD_ID") {
  try {
    return readFileSync(path, "utf8").trim() || null;
  } catch {
    // `sw:compile` run without a preceding `next build` (dev tooling).
    return null;
  }
}

export function stampEntries(entries, id) {
  return entries.map((entry) => ({
    ...entry,
    url: `/${entry.url}`,
    revision: id && entry.revision ? `${entry.revision}-${id}` : entry.revision,
  }));
}

const config = {
  swSrc: "app/sw.ts",
  swDest: "public/sw.js",
  injectionPoint: "self.__SW_MANIFEST",
  globDirectory: "public",
  globPatterns: ["offline.html"],
  manifestTransforms: [
    async (entries) => ({ manifest: stampEntries(entries, buildId()) }),
  ],
};

export default config;
