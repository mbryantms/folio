import type { BatchLookupCount, FallbackReason } from "@/lib/api/types";
import { providerLabel } from "@/lib/metadata/quota";

/** Why a batch issue searched instead of using its series' coverage. */
const FALLBACK_LABELS: Record<FallbackReason, string> = {
  no_target: "no provider series",
  list_unavailable: "issue list unavailable",
  not_listed: "number not listed",
  date_conflict: "cover date conflict",
  detail_unavailable: "detail fetch failed",
  rejected_by_matcher: "rejected by matcher",
};

/** `"ComicVine: 170 direct · 3 searched (2 number not listed, 1 cover date conflict)"`. */
export function lookupLine(l: BatchLookupCount): string {
  const base = `${providerLabel(l.source)}: ${l.direct} direct · ${l.search} searched`;
  if (l.fallbacks.length === 0) return base;
  const why = l.fallbacks
    .map((f) => `${f.count} ${FALLBACK_LABELS[f.reason] ?? f.reason}`)
    .join(", ");
  return `${base} (${why})`;
}

/**
 * Per-provider direct-lookup vs search tally for a metadata batch: issues a
 * provider answered from its series' issue list (series coverage) skip the
 * provider search entirely. Renders nothing until a child has searched.
 */
export function BatchLookupSummary({
  lookups,
}: {
  lookups: BatchLookupCount[];
}) {
  if (lookups.length === 0) return null;
  return (
    <div className="text-muted-foreground space-y-0.5 text-xs">
      <p>Provider lookups (direct = from series coverage, no search):</p>
      <ul aria-label="Provider lookups" className="space-y-0.5">
        {lookups.map((l) => (
          <li key={l.source} className="tabular-nums">
            {lookupLine(l)}
          </li>
        ))}
      </ul>
    </div>
  );
}
