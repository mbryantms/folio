/** Compile separately from Next so production can keep using Turbopack.
 * Only the public offline document is precached. Hashed Next assets are
 * cached on demand; visiting the library does not download admin bundles.
 */
const config = {
  swSrc: "app/sw.ts",
  swDest: "public/sw.js",
  injectionPoint: "self.__SW_MANIFEST",
  globDirectory: "public",
  globPatterns: ["offline.html"],
  manifestTransforms: [
    async (entries) => ({
      manifest: entries.map((entry) => ({ ...entry, url: `/${entry.url}` })),
    }),
  ],
};

export default config;
