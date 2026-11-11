import Link from "next/link";
import { AlertTriangle } from "lucide-react";

import { cn } from "@/lib/utils";
import { statusToneText } from "@/lib/ui/status-tone";

/**
 * Files a scan run flagged as findings, read from the run's persisted
 * `stats` counters. Each malformed (archive or ComicInfo), encrypted and
 * duplicate file the scanner counts also opens one health finding
 * (`scanner/process.rs`), so a non-zero sum means the run produced
 * findings. It is a lower bound, not the run's findings total: kinds the
 * scanner records without a counter (empty folders, files at the root,
 * missing ComicInfo, …) are not in `stats`, and the list response has no
 * per-run findings count nor the findings endpoint a run filter.
 */
export function scanRunFlaggedFiles(stats: unknown): number {
  if (!stats || typeof stats !== "object") return 0;
  const s = stats as Record<string, unknown>;
  const num = (k: string): number =>
    typeof s[k] === "number" && Number.isFinite(s[k]) ? (s[k] as number) : 0;
  return (
    num("files_malformed") + num("files_encrypted") + num("files_duplicate")
  );
}

/** Compact "N flagged" link from a scan row to the findings it opened.
 *  Renders nothing when the run flagged no files. */
export function ScanRunFindingsLink({
  stats,
  href,
  className,
}: {
  stats: unknown;
  /** The findings view for the run's library. */
  href: string;
  className?: string;
}) {
  const count = scanRunFlaggedFiles(stats);
  if (count <= 0) return null;
  const files = count === 1 ? "file" : "files";
  return (
    <Link
      href={href}
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
      {count} {files} flagged
    </Link>
  );
}
