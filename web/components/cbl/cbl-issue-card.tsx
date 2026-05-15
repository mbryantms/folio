"use client";

import Link from "next/link";
import { useRouter } from "next/navigation";

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
import { cn } from "@/lib/utils";
import type { CblEntryView, IssueSummaryView } from "@/lib/api/types";
import { issueUrl, readerUrl } from "@/lib/urls";

/** CBL-rail variant of `IssueCard` — same cover treatment but adds a
 *  position badge for the `<Books>` order and surfaces the entry's
 *  match-status when the underlying issue isn't resolved. */
export function CblIssueCard({
  entry,
  issue,
  cblSavedViewId,
  className,
}: {
  entry: CblEntryView;
  /** Hydrated issue when `entry.matched_issue_id` resolves to a row in
   *  the user's library. Missing/ambiguous entries pass `undefined`
   *  here and the card renders the entry's raw metadata as a placeholder. */
  issue?: IssueSummaryView;
  /** Saved-view id of the CBL this card belongs to. Threaded into the
   *  reader URL as `?cbl=` so the reader's next-issue resolver picks
   *  the next list entry instead of the next series issue. */
  cblSavedViewId: string;
  className?: string;
}) {
  const positionLabel = `#${entry.position + 1}`;
  const numberLabel = entry.issue_number ? `#${entry.issue_number}` : "—";
  const heading = issue?.title ?? entry.series_name;
  const router = useRouter();
  const upsertProgress = useUpsertIssueProgress();
  const collectionActions = useCoverMenuCollectionActions({
    entry_kind: "issue",
    ref_id: issue?.id ?? "",
    label: `${heading} ${numberLabel}`,
  });
  const menuActions: CoverMenuAction[] =
    issue && issue.state === "active"
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
  // Hook is always called (rules-of-hooks); when there's no resolved
  // issue the sheet renders nothing useful, which is fine.
  const longPress = useCoverLongPressActions({
    primary:
      issue && issue.state === "active"
        ? {
            label: `Read ${heading}`,
            onSelect: () =>
              router.push(readerUrl(issue, { cbl: cblSavedViewId })),
          }
        : undefined,
    actions: menuActions,
    label: `${positionLabel} · ${heading}`,
  });

  const cover = (showActions: boolean) => (
    <div className="relative" {...longPress.wrapperProps}>
      <Cover
        src={issue?.cover_url}
        alt={heading}
        fallback={numberLabel}
        className={cn(
          "w-full transition group-hover:brightness-110",
          !issue && "opacity-60",
        )}
      />
      {/* Position badge moved to bottom-left so top-left is free for the
       *  kebab affordance. Match-status badge stays at top-right (it's
       *  the highest-priority signal for unresolved entries). */}
      <Badge
        variant="secondary"
        className="bg-background/80 absolute bottom-2 left-2 backdrop-blur"
      >
        {positionLabel}
      </Badge>
      {entry.match_status !== "matched" && entry.match_status !== "manual" && (
        <Badge
          variant="destructive"
          className="bg-background/80 absolute top-2 right-2 backdrop-blur"
        >
          {entry.match_status}
        </Badge>
      )}
      {showActions && issue && (
        <>
          <CoverMenuButton
            label={`Actions for ${heading}`}
            actions={menuActions}
          />
          <QuickReadOverlay
            readerHref={readerUrl(issue, { cbl: cblSavedViewId })}
            label={`Read ${heading}`}
          />
        </>
      )}
    </div>
  );
  const meta = (
    <div className="min-w-0 px-1">
      <div className="text-muted-foreground text-xs font-medium">
        {numberLabel}
        {entry.year ? ` · ${entry.year}` : null}
      </div>
      <div className="truncate text-sm font-medium" title={heading}>
        {heading}
      </div>
    </div>
  );
  const wrapClass = cn(
    "group hover:bg-accent/40 focus-visible:ring-ring flex flex-col gap-2 rounded-md p-1 transition-colors focus-visible:ring-2 focus-visible:outline-none",
    className,
  );
  // Dialog must render outside <Link> — see SeriesCard for the bubble
  // explanation. Wrap in a fragment so the dialog ends up as a Link
  // sibling rather than a child.
  if (issue) {
    return (
      <>
        <Link
          href={issueUrl(issue, { cbl: cblSavedViewId })}
          className={wrapClass}
        >
          {cover(issue.state === "active")}
          {meta}
        </Link>
        {collectionActions.dialog}
        {longPress.sheet}
      </>
    );
  }
  return (
    <div className={cn(wrapClass, "cursor-default opacity-90")}>
      {cover(false)}
      {meta}
    </div>
  );
}
