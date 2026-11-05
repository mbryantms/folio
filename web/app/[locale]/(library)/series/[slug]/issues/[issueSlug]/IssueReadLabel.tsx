"use client";

import { useIssueReadState } from "@/lib/api/mutations";
import { readButtonLabel, type ReadState } from "@/lib/reading-state";

/**
 * The issue page's Read CTA label ("Read" / "Continue reading" / "Read
 * again"), live: marking the issue read or unread (menu, `r`/`u`, Undo)
 * relabels it at once instead of after the RSC refresh. `readState` is
 * the server-rendered state it starts from.
 */
export function IssueReadLabel({
  issue,
  readState,
}: {
  issue: { id: string; page_count?: number | null };
  readState: ReadState;
}) {
  return <>{readButtonLabel(useIssueReadState(issue, readState))}</>;
}
