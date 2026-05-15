"use client";

import Link from "next/link";
import { useRouter } from "next/navigation";
import { Check } from "lucide-react";

import { Cover } from "@/components/Cover";
import {
  CoverMenuButton,
  type CoverMenuAction,
} from "@/components/CoverMenuButton";
import { useCoverLongPressActions } from "@/components/CoverLongPressActions";
import { useCoverMenuCollectionActions } from "@/components/collections/useCoverMenuCollectionActions";
import { QuickReadOverlay } from "@/components/QuickReadOverlay";
import { Badge } from "@/components/ui/badge";
import { useUpsertIssueProgress } from "@/lib/api/mutations";
import type { CblWindowEntry } from "@/lib/api/types";
import { formatIssueHeading } from "@/lib/format";
import { issueUrl, readerUrl } from "@/lib/urls";
import { cn } from "@/lib/utils";

/**
 * Card for one entry inside the CBL home rail's reading window. Combines
 * the position badge from `CblIssueCard` with the progress bar from
 * `ProgressIssueCard`, plus a status treatment that depends on where
 * this entry sits relative to the user's current up-next item:
 *
 *  - **Finished** entries get a subtle dim + checkmark so the user can
 *    see at a glance what they've already read.
 *  - **The current entry** (the next to read) gets a primary-tinted
 *    ring so it stands out as the anchor of the rail.
 *  - **In-progress** entries (rare in a CBL rail — most picks bounce
 *    between unread and finished) render their progress bar like the
 *    Continue Reading rail.
 *  - **Upcoming** entries render plain.
 *
 * Cover-click goes to the issue detail page (matching the cross-card
 * convention); only the yellow play overlay routes straight to the
 * reader.
 */
export function CblWindowCard({
  entry,
  isCurrent,
  cblSavedViewId,
  className,
}: {
  entry: CblWindowEntry;
  /** True when this entry is the user's next-to-read in the CBL — adds
   *  a ring + slight scale so the rail visually anchors on it. */
  isCurrent: boolean;
  /** Saved-view id of the CBL this rail belongs to. Threaded into the
   *  reader URL as `?cbl=` so the next-issue resolver picks the next
   *  list entry instead of the next series issue. */
  cblSavedViewId: string;
  className?: string;
}) {
  const upsertProgress = useUpsertIssueProgress();
  const router = useRouter();
  const issue = entry.issue;
  const numberLabel = issue.number ? `#${issue.number}` : "—";
  const heading = formatIssueHeading(issue);
  const positionLabel = `#${entry.position + 1}`;
  const collectionActions = useCoverMenuCollectionActions({
    entry_kind: "issue",
    ref_id: issue.id,
    label: `${heading} ${numberLabel}`,
  });

  const inProgress = !entry.finished && entry.last_page > 0;
  const percent = Math.max(0, Math.min(100, Math.round(entry.percent * 100)));

  const menuActions: CoverMenuAction[] =
    issue.state === "active"
      ? [
          {
            label: "Mark as read",
            onSelect: () =>
              upsertProgress.mutate({
                issue_id: issue.id,
                page: Math.max((issue.page_count ?? 1) - 1, 0),
                finished: true,
              }),
          },
          {
            label: "Mark as unread",
            onSelect: () =>
              upsertProgress.mutate({
                issue_id: issue.id,
                page: 0,
                finished: false,
              }),
          },
          ...collectionActions.actions,
        ]
      : [];
  const primaryLabel = isCurrent
    ? `Continue reading ${heading}`
    : entry.finished
      ? `Re-read ${heading}`
      : `Read ${heading}`;
  const longPress = useCoverLongPressActions({
    primary:
      issue.state === "active"
        ? {
            label: primaryLabel,
            onSelect: () =>
              router.push(readerUrl(issue, { cbl: cblSavedViewId })),
          }
        : undefined,
    actions: menuActions,
    label: `${positionLabel} · ${heading}`,
  });

  // Dialog must render outside <Link> — see SeriesCard for why.
  return (
    <>
      <Link
        href={issueUrl(issue, { cbl: cblSavedViewId })}
        className={cn(
          "group hover:bg-accent/40 focus-visible:ring-ring flex flex-col gap-2 rounded-md p-1 transition-colors focus-visible:ring-2 focus-visible:outline-none",
          className,
        )}
      >
        <div
          className={cn(
            "relative rounded-md transition-all",
            // Current-entry ring sits on the cover wrapper (not the whole
            // card) so the surrounding padding/title stay neutral.
            isCurrent &&
              "ring-primary ring-offset-background ring-2 ring-offset-2",
          )}
          {...longPress.wrapperProps}
        >
          <Cover
            src={issue.cover_url}
            alt={heading}
            fallback={numberLabel}
            className={cn(
              "w-full transition group-hover:brightness-110",
              entry.finished && "opacity-50",
            )}
          />
          <Badge
            variant="secondary"
            className="bg-background/80 absolute bottom-2 left-2 backdrop-blur"
          >
            {positionLabel}
          </Badge>
          {entry.finished && (
            <span
              aria-label="Already read"
              title="Already read"
              className="bg-primary/90 text-primary-foreground absolute top-2 right-2 inline-flex h-7 w-7 items-center justify-center rounded-full ring-2 shadow-md ring-white/20 backdrop-blur"
            >
              <Check aria-hidden="true" className="h-4 w-4" />
            </span>
          )}
          {inProgress && (
            <div
              className="bg-background/70 absolute inset-x-0 bottom-0 h-1.5 overflow-hidden rounded-b-md"
              aria-hidden="true"
            >
              <div
                className="bg-primary h-full transition-[width]"
                style={{ width: `${percent}%` }}
              />
            </div>
          )}
          {issue.state === "active" && (
            <>
              <CoverMenuButton
                label={`Actions for ${heading}`}
                actions={menuActions}
              />
              <QuickReadOverlay
                readerHref={readerUrl(issue, { cbl: cblSavedViewId })}
                label={primaryLabel}
              />
            </>
          )}
        </div>
        <div className="min-w-0 px-1">
          <div className="text-muted-foreground text-xs font-medium">
            {numberLabel}
            {isCurrent && (
              <span className="text-primary ml-1.5 text-[10px] font-semibold tracking-wider uppercase">
                Up next
              </span>
            )}
          </div>
          <div className="truncate text-sm font-medium" title={heading}>
            {heading}
          </div>
        </div>
      </Link>
      {collectionActions.dialog}
      {longPress.sheet}
    </>
  );
}
