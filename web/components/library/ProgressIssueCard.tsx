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
import { formatIssueHeading } from "@/lib/format";
import { cn } from "@/lib/utils";
import type { ContinueReadingCard } from "@/lib/api/types";
import { issueUrl, readerUrl } from "@/lib/urls";
import {
  useDismissRailItem,
  useUpsertIssueProgress,
} from "@/lib/api/mutations";

/**
 * Continue Reading card. Shows the issue cover with a thin progress bar
 * at the bottom, the parent series name + issue number above the title,
 * and the shared kebab + play affordances on hover.
 *
 * `primaryAction` decides where the card's main tap goes. The Continue
 * Reading *rail* resumes the issue in the reader (the issue auto-resumes
 * at `last_page` server-side) and offers "Issue details" as a secondary
 * link + sheet action (post-audit T2.1, D1); the rail's "View all" page
 * and any other surface keep the issue detail page as the primary
 * target, like every other cover-bearing card. The kebab menu offers
 * mark-read / mark-unread / hide-from-rail, matching the cross-card
 * affordance contract from `CoverMenuButton`.
 */
export function ProgressIssueCard({
  card,
  className,
  primaryAction = "details",
}: {
  card: ContinueReadingCard;
  className?: string;
  primaryAction?: "reader" | "details";
}) {
  const dismiss = useDismissRailItem();
  const upsertProgress = useUpsertIssueProgress();
  const router = useRouter();

  const issue = card.issue;
  const pageCount = issue.page_count ?? 0;
  const percent = Math.max(
    0,
    Math.min(100, Math.round(card.progress.percent * 100)),
  );
  const numberLabel = issue.number ? `#${issue.number}` : "—";
  const headerTitle = formatIssueHeading(issue, card.series_name);
  const collectionActions = useCoverMenuCollectionActions({
    entry_kind: "issue",
    ref_id: issue.id,
    label: `${headerTitle} ${numberLabel}`,
  });
  const resumeHref = readerUrl(issue);
  const detailsHref = issueUrl(issue);
  const resumesOnTap = primaryAction === "reader";
  const menuActions: CoverMenuAction[] = [
    ...(resumesOnTap
      ? [
          {
            label: "Issue details",
            onSelect: () => router.push(detailsHref),
          },
        ]
      : []),
    {
      label: "Mark as read",
      onSelect: () =>
        upsertProgress.mutate({
          issue_id: issue.id,
          page: pageCount > 0 ? pageCount - 1 : 0,
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
    {
      label: "Hide from rail",
      onSelect: () =>
        dismiss.mutate({
          target_kind: "issue",
          target_id: issue.id,
        }),
    },
    ...collectionActions.actions,
  ];
  const longPress = useCoverLongPressActions({
    primary: {
      label: `Continue reading ${headerTitle}`,
      onSelect: () => router.push(resumeHref),
    },
    actions: menuActions,
    label: `${card.series_name} · ${numberLabel}`,
  });

  // Cover/title click → the primary target: the reader on the Continue
  // Reading rail (a touch user had no way to resume except the long-
  // press sheet — the play overlay is hover-only), the issue detail page
  // everywhere else. The yellow play overlay always routes to the reader.
  //
  // Collection-actions dialog renders as a sibling of <Link>, not a
  // child — React synthetic events bubble through the React tree even
  // across portals, so a click inside the dialog would otherwise
  // propagate to the Link and route away.
  return (
    <div className={cn("flex flex-col", className)}>
      <Link
        href={resumesOnTap ? resumeHref : detailsHref}
        aria-label={
          resumesOnTap
            ? `Continue reading ${headerTitle}`
            : `${headerTitle} details`
        }
        className={cn(
          "group hover:bg-accent/40 focus-visible:ring-ring flex flex-col gap-2 rounded-md p-1 transition-colors focus-visible:ring-2 focus-visible:outline-none",
        )}
      >
        <div className="relative" {...longPress.wrapperProps}>
          <Cover
            src={issue.cover_url}
            alt={headerTitle}
            fallback={numberLabel}
            className="w-full transition group-hover:brightness-110"
          />
          {/* Progress overlay: thin bar at the bottom of the cover. The
           *  bar always shows (not hover-gated) since it's a key signal
           *  for this rail's purpose. */}
          <div
            className="bg-background/70 absolute inset-x-0 bottom-0 h-1.5 overflow-hidden rounded-b-md"
            aria-hidden="true"
          >
            <div
              className="bg-primary h-full transition-[width]"
              style={{ width: `${percent}%` }}
            />
          </div>
          <CoverMenuButton
            label={`Actions for ${headerTitle}`}
            actions={menuActions}
          />
          <QuickReadOverlay
            readerHref={resumeHref}
            label={`Continue reading ${headerTitle}`}
          />
        </div>
        <div className="min-w-0 px-1">
          <div
            className="text-muted-foreground truncate text-xs"
            title={card.series_name}
          >
            {card.series_name} · {numberLabel}
          </div>
          <div className="truncate text-sm font-medium" title={headerTitle}>
            {headerTitle}
          </div>
          <div className="text-muted-foreground mt-0.5 text-[11px]">
            {pageCount > 0
              ? `Page ${card.progress.last_page + 1} of ${pageCount}`
              : `${percent}%`}
          </div>
        </div>
      </Link>
      {resumesOnTap ? (
        <Link
          href={detailsHref}
          className="text-muted-foreground hover:text-foreground focus-visible:ring-ring mx-1 w-fit rounded-sm px-1 text-[11px] underline-offset-4 hover:underline focus-visible:ring-2 focus-visible:outline-none"
        >
          Details
        </Link>
      ) : null}
      {collectionActions.dialog}
      {longPress.sheet}
    </div>
  );
}

export function ProgressIssueCardSkeleton({
  className,
}: {
  className?: string;
}) {
  return (
    <div className={cn("flex flex-col gap-2 p-1", className)}>
      <div className="bg-muted aspect-[2/3] w-full animate-pulse rounded-md" />
      <div className="space-y-1.5 px-1">
        <div className="bg-muted h-2 w-1/2 animate-pulse rounded" />
        <div className="bg-muted h-3 w-3/4 animate-pulse rounded" />
        <div className="bg-muted h-2 w-1/3 animate-pulse rounded" />
      </div>
    </div>
  );
}
