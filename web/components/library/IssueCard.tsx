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
import { useUserProgress } from "@/lib/api/queries";
import { formatIssueHeading } from "@/lib/format";
import { cn } from "@/lib/utils";
import type { IssueSummaryView } from "@/lib/api/types";
import { issueUrl, readerUrl } from "@/lib/urls";

export function IssueCard({
  issue,
  className,
  extraActions,
}: {
  issue: IssueSummaryView;
  className?: string;
  /** Appended to the cover-menu's default actions. Use for
   *  surface-specific affordances (e.g. "Remove from this collection"
   *  on the collection detail page). */
  extraActions?: CoverMenuAction[];
}) {
  const numberLabel = issue.number ? `#${issue.number}` : "—";
  const heading = formatIssueHeading(issue);
  const router = useRouter();
  const upsertProgress = useUpsertIssueProgress();
  // Shared `/progress` query — one network call regardless of grid size,
  // TanStack dedupes by queryKey. Absent (undefined) means unread or
  // still loading; we render no badge in either case so the cover
  // doesn't flicker once data arrives.
  const progressMap = useUserProgress().data;
  const progress = progressMap?.get(issue.id);
  const finished = progress?.finished ?? false;
  const inProgress =
    !!progress && !finished && progress.percent > 0;
  const percent = inProgress
    ? Math.max(0, Math.min(100, Math.round(progress.percent * 100)))
    : 0;
  const collectionActions = useCoverMenuCollectionActions({
    entry_kind: "issue",
    ref_id: issue.id,
    label: `${heading}${issue.number ? ` ${numberLabel}` : ""}`,
  });
  // Same actions backing the desktop kebab — sharing the array means
  // touch (long-press sheet) and desktop (dropdown) can't drift.
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
          ...(extraActions ?? []),
        ]
      : [];
  const longPress = useCoverLongPressActions({
    primary:
      issue.state === "active"
        ? {
            label: `Read ${heading}`,
            onSelect: () => router.push(readerUrl(issue)),
          }
        : undefined,
    actions: menuActions,
    label: heading,
  });
  // State badge moves out of the top-left corner when the kebab is
  // present (kebab is hover-revealed; badge is always-on, so the kebab
  // covers it briefly on hover — same trade-off the other rails make).
  //
  // The collection-actions dialog renders as a sibling of the <Link>,
  // not a child: React synthetic events bubble through the React tree
  // even across portals, so a click inside the dialog would otherwise
  // propagate to the Link's onClick and route the user away.
  return (
    <>
      <Link
        href={issueUrl(issue)}
        className={cn(
          "group hover:bg-accent/40 focus-visible:ring-ring flex flex-col gap-2 rounded-md p-1 transition-colors focus-visible:ring-2 focus-visible:outline-none",
          className,
        )}
      >
        <div className="relative" {...longPress.wrapperProps}>
          <Cover
            src={issue.cover_url}
            alt={heading}
            fallback={numberLabel}
            className="w-full transition group-hover:brightness-110"
          />
          {issue.state !== "active" && (
            <Badge
              variant="destructive"
              className="bg-background/80 absolute top-2 right-2 backdrop-blur"
            >
              {issue.state}
            </Badge>
          )}
          {/* Read state — bottom-left mirrors `CollectionDot` on series
           *  cards (one "status indicator" slot per card type). No
           *  cover dimming on browse surfaces: the library/home rails
           *  want covers to stay vibrant. */}
          {issue.state === "active" && finished && (
            <span
              aria-label="Read"
              title="Read"
              className="bg-primary/90 text-primary-foreground absolute bottom-2 left-2 inline-flex h-6 w-6 items-center justify-center rounded-full ring-1 shadow-sm ring-black/10 backdrop-blur dark:ring-white/10"
            >
              <Check aria-hidden="true" className="h-3.5 w-3.5" />
            </span>
          )}
          {issue.state === "active" && inProgress && (
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
                readerHref={readerUrl(issue)}
                label={`Read ${heading}`}
              />
            </>
          )}
        </div>
        <div className="min-w-0 px-1">
          <div className="text-muted-foreground text-xs font-medium">
            {numberLabel}
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

export function IssueCardSkeleton({ className }: { className?: string }) {
  return (
    <div className={cn("flex flex-col gap-2 p-1", className)}>
      <div className="bg-muted aspect-[2/3] w-full animate-pulse rounded-md" />
      <div className="space-y-1.5 px-1">
        <div className="bg-muted h-2 w-1/3 animate-pulse rounded" />
        <div className="bg-muted h-3 w-3/4 animate-pulse rounded" />
      </div>
    </div>
  );
}
