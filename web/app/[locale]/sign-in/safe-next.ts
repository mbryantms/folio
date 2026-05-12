/**
 * Safe-redirect helper for the `next` search-param. Mirrors the
 * server-side `is_safe_redirect_target` guard in `auth/oidc.rs` —
 * rejects anything that isn't a single-leading-slash path, has
 * `//` / `://` / `\\`, or carries a colon in the path portion.
 */
export function isSafeNextPath(
  next: string | null | undefined,
): next is string {
  if (!next) return false;
  if (!next.startsWith("/")) return false;
  if (next.startsWith("//") || next.startsWith("/\\")) return false;
  if (next.includes("://") || next.includes("\\")) return false;
  const noQuery = next.split("?")[0] ?? next;
  const path = noQuery.split("#")[0] ?? noQuery;
  if (path.includes(":")) return false;
  for (let i = 0; i < next.length; i++) {
    const cc = next.charCodeAt(i);
    if (cc < 0x20 || cc === 0x7f) return false;
  }
  return true;
}
