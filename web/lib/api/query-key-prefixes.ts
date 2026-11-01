/**
 * Invalidation-only key prefixes over the `queryKeys` registry
 * (`./query-keys.ts`): each one matches every key `queryKeys` builds
 * under it. Lives apart from `queryKeys` because the reader imports that
 * object whole and its first-load budget has no room for keys only
 * library-side mutations use (remediation B9). Add a prefix here only
 * when the matching `queryKeys` entries already exist.
 */
export const queryKeyPrefixes = {
  /** Every series query: detail, list, issues, collection report. */
  series: ["series"] as const,
  /** Every saved-view query: index, detail, results. */
  savedViews: ["saved-views"] as const,
  /** Every home-rail query. */
  rails: ["rails"] as const,
  /** Every collection query: list, detail, entries. */
  collections: ["collections"] as const,
  /** Every entity-page query: search, insights, grids. */
  entities: ["entities"] as const,
  /** Both similar-series queries: series tab and home rail. */
  similar: ["similar"] as const,
};
