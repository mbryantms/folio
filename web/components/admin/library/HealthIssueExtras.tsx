import Link from "next/link";
import { ExternalLink } from "lucide-react";

import {
  healthKindHint,
  healthPayloadDetails,
  healthPayloadRefs,
} from "@/lib/library/health-issues";

/** How many series links a drift row renders before truncating with a
 *  "+N more" note — a library-wide drift can reference hundreds. */
const MAX_SERIES_LINKS = 5;

/** Jump-to-item links for a health row's payload refs — the fix action
 *  usually lives on the issue/series page, so hiding rows shouldn't be
 *  the path of least resistance (audit UX-3). */
export function PayloadRefLinks({ payload }: { payload: unknown }) {
  const refs = healthPayloadRefs(payload);
  if (refs.seriesIds.length === 0 && !refs.issueId) return null;
  const linkCls =
    "text-foreground/80 hover:text-foreground inline-flex items-center gap-1 text-xs underline-offset-2 hover:underline";
  const shown = refs.seriesIds.slice(0, MAX_SERIES_LINKS);
  const extra = refs.seriesIds.length - shown.length;
  return (
    <span className="mt-1 flex flex-wrap gap-3">
      {refs.issueId ? (
        <Link href={`/issues/${refs.issueId}`} className={linkCls}>
          <ExternalLink className="size-3" aria-hidden="true" />
          View issue
        </Link>
      ) : null}
      {shown.map((id, i) => (
        <Link key={id} href={`/series/${id}`} className={linkCls}>
          <ExternalLink className="size-3" aria-hidden="true" />
          {shown.length > 1 ? `View series ${i + 1}` : "View series"}
        </Link>
      ))}
      {extra > 0 ? (
        <span className="text-muted-foreground text-xs">+{extra} more</span>
      ) : null}
    </span>
  );
}

/** Fix hint + expandable list (skipped-subtree preview, per-value series
 *  breakdown) for kinds that carry one. Renders nothing otherwise. */
export function HealthPayloadExtras({
  kind,
  payload,
}: {
  kind: string;
  payload: unknown;
}) {
  const hint = healthKindHint(kind);
  const details = healthPayloadDetails(kind, payload);
  if (!hint && !details) return null;
  return (
    <span className="mt-1 block space-y-1">
      {hint ? (
        <span className="text-muted-foreground block text-xs">{hint}</span>
      ) : null}
      {details ? (
        <details className="text-xs">
          <summary className="text-foreground/80 hover:text-foreground cursor-pointer select-none">
            {details.title} ({details.lines.length + details.more})
          </summary>
          <ul className="text-muted-foreground mt-1 space-y-0.5 pl-4 font-mono wrap-anywhere">
            {details.lines.map((line) => (
              <li key={line}>{line}</li>
            ))}
            {details.more > 0 ? (
              <li className="font-sans italic">…and {details.more} more</li>
            ) : null}
          </ul>
        </details>
      ) : null}
    </span>
  );
}
