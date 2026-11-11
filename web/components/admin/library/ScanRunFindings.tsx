import Link from "next/link";
import { AlertTriangle } from "lucide-react";

import { cn } from "@/lib/utils";
import { statusToneText } from "@/lib/ui/status-tone";

/** The findings page filtered server-side to one scan run's findings. */
export function scanRunFindingsHref(scanId: string): string {
  return `/admin/findings?tab=health&scan_id=${encodeURIComponent(scanId)}`;
}

/** Compact "N findings" link from a scan row to the open health findings
 *  the run recorded (`health_issue_count`, counted by the server). Renders
 *  nothing when the run has none, like the row's other empty cells. */
export function ScanRunFindingsLink({
  scanId,
  count,
  className,
}: {
  scanId: string;
  /** `ScanRunView.health_issue_count`. */
  count: number;
  className?: string;
}) {
  if (!Number.isFinite(count) || count <= 0) return null;
  const noun = count === 1 ? "finding" : "findings";
  return (
    <Link
      href={scanRunFindingsHref(scanId)}
      // Stop the bubble so an expandable row does not toggle as well.
      onClick={(e) => e.stopPropagation()}
      data-testid="scan-run-findings"
      className={cn(
        "inline-flex items-center gap-1 text-xs font-medium underline-offset-2 hover:underline",
        statusToneText("warning"),
        className,
      )}
    >
      <AlertTriangle className="size-3.5 shrink-0" aria-hidden="true" />
      {count} {noun}
    </Link>
  );
}
