/**
 * Build an API path from a template literal, encoding every interpolated
 * value as one URI component:
 *
 *   apiPath`/series/${slug}/issues/${issueSlug}` + buildQuery(filters)
 *
 * A route param or other value containing `/`, `?`, `#` or `%` then stays
 * inside its own segment instead of addressing a different API path.
 * Encoding is unconditional, so an already-encoded value is encoded again
 * (`a%20b` → `a%2520b`): pass raw values, never `encodeURIComponent(x)`.
 * A whole query string (`buildQuery(...)`, `?${qs}`) is appended outside
 * the template, since its `?`, `&` and `=` must stay as they are; a single
 * query value inside the template (`?range=${range}`) is encoded like a
 * segment, which is the correct encoding for a value.
 *
 * Route params are the one value that does NOT arrive raw: the App
 * Router hands a page its dynamic params already percent-encoded
 * (`next/dist/shared/lib/router/utils/get-dynamic-param.js`). Pass them
 * through {@link routeParam} first so they are encoded exactly once.
 *
 * Shared by the server loaders (`apiGet` in `./fetch`) and the client
 * query and mutation hooks, so it imports nothing.
 */
export function apiPath(
  strings: TemplateStringsArray,
  ...values: ReadonlyArray<string | number>
): string {
  let out = strings[0] ?? "";
  values.forEach((value, i) => {
    out += encodeURIComponent(String(value)) + (strings[i + 1] ?? "");
  });
  return out;
}

/**
 * A dynamic route param as a page or layout receives it from the App
 * Router: percent-encoded. Returns the decoded value, ready for
 * {@link apiPath} (which encodes every value) or for display. Folio's
 * slugs are ASCII (`[a-z0-9-]`), so this is normally the identity; it
 * matters only if a slug ever carries a character that
 * `encodeURIComponent` changes. A value that is not valid
 * percent-encoding is returned unchanged.
 */
export function routeParam(value: string): string {
  try {
    return decodeURIComponent(value);
  } catch {
    return value;
  }
}
