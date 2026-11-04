/**
 * Page cross-fade key (folio-flair, standard).
 *
 * The shells cross-fade their content column when the user moves between
 * *top-level* destinations: the things the sidebar and the bottom tab bar
 * link to. Moving within one (a series to another series, an issue to the
 * next, filter changes on a grid) keeps the key, so it doesn't fade; the
 * cover morph covers the card-to-detail hop.
 *
 * - `/` splits by `?library=`: Home, All libraries and each library are
 *   separate sidebar entries.
 * - Sections whose second segment is itself a destination (a saved view,
 *   a custom page, a settings or admin page) key on two segments.
 * - Everything else keys on its first segment.
 */
const TWO_SEGMENT_ROOTS = new Set(["views", "pages", "settings", "admin"]);

export function routeFadeKey(
  pathname: string,
  library: string | null | undefined,
): string {
  const segs = pathname.split("/").filter(Boolean);
  const first = segs[0];
  if (!first) return library ? `/?library=${library}` : "/";
  if (TWO_SEGMENT_ROOTS.has(first)) return `/${first}/${segs[1] ?? ""}`;
  return `/${first}`;
}
