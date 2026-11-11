/** Serialise query params, skipping `undefined` / empty values. Shared by
 *  the client hooks (`./queries`) and the shared query options
 *  (`./query-options`), which a server component may import. */
export function buildQuery(
  params: Record<string, string | number | undefined>,
): string {
  const sp = new URLSearchParams();
  for (const [k, v] of Object.entries(params)) {
    if (v === undefined || v === "" || v === null) continue;
    sp.set(k, String(v));
  }
  const s = sp.toString();
  return s ? `?${s}` : "";
}
