/** Only these public assets may enter a service-worker runtime cache. */
export const STATIC_PATH = /^\/_next\/static\/.*\.(?:js|css|woff2?)$/;
export const ICON_PATH = /^\/icons\/[^/]+\.(?:png|svg|ico)$/;
export const THUMB_PATH =
  /^\/issues\/[^/]+\/(?:pages\/\d+\/thumb|covers\/[^/]+)$/;
export const THUMB_CACHE = "folio-thumbs-v3";
export const LEGACY_CACHES = [
  "apis",
  "others",
  "pages",
  "pages-rsc",
  "pages-rsc-prefetch",
  "next-data",
  "static-data-assets",
  "folio-thumbs",
  "folio-thumbs-v2",
];
export function isPublicAsset(path: string): boolean {
  return STATIC_PATH.test(path) || ICON_PATH.test(path);
}
