"use client";

import { memo } from "react";
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
import { SelectionCheckbox } from "@/components/library/SelectionCheckbox";
import { Badge } from "@/components/ui/badge";
import { useUpsertIssueProgress } from "@/lib/api/mutations";
import { useUserProgress } from "@/lib/api/queries";
import { formatIssueHeading } from "@/lib/format";
import { cn } from "@/lib/utils";
import type { IssueSummaryView } from "@/lib/api/types";
import { issueUrl, readerUrl } from "@/lib/urls";
import { useShareLink } from "@/lib/ui/use-share-link";

function IssueCardImpl({
  issue,
  className,
  extraActions,
  selectMode,
  onEnterSelectMode,
}: {
  issue: IssueSummaryView;
  className?: string;
  /** Appended to the cover-menu's default actions. Use for
   *  surface-specific affordances (e.g. "Remove from this collection"
   *  on the collection detail page). */
  extraActions?: CoverMenuAction[];
  /** When the parent surface is in multi-select mode. When set:
   *  - The outer `<Link>` becomes a `<button>` and clicking
   *    toggles selection instead of navigating.
   *  - `useCoverLongPressActions`'s `wrapperProps` are NOT spread
   *    onto the cover, so long-press doesn't open the existing
   *    actions sheet inside select mode.
   *  - A `<SelectionCheckbox>` is rendered as an overlay.
   *
   *  Plan: `~/.claude/plans/multi-select-bulk-actions-1.0.md` (M1).
   */
  selectMode?: {
    isActive: boolean;
    isSelected: boolean;
    onToggle: (ev?: React.MouseEvent) => void;
  };
  /** Optional callback for the long-press sheet's "Select" entry.
   *  When set, the long-press menu gets an action that enters the
   *  parent's select mode and pre-selects this card. When unset
   *  (e.g. on home rails that don't support multi-select), the
   *  entry isn't rendered. Plan: M6. */
  onEnterSelectMode?: (id: string) => void;
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
  const inProgress = !!progress && !finished && progress.percent > 0;
  const percent = inProgress
    ? Math.max(0, Math.min(100, Math.round(progress.percent * 100)))
    : 0;
  const collectionActions = useCoverMenuCollectionActions({
    entry_kind: "issue",
    ref_id: issue.id,
    label: `${heading}${issue.number ? ` ${numberLabel}` : ""}`,
  });
  const share = useShareLink();
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
          {
            label: share.label,
            onSelect: () => void share.shareOrCopy(issueUrl(issue), heading),
          },
          ...(extraActions ?? []),
        ]
      : [];
  // Long-press sheet's actions = the desktop kebab's actions + an
  // optional leading "Select" entry. The Select entry only appears
  // when the parent surface wired `onEnterSelectMode` AND we're not
  // already in select mode (in which case the long-press handler
  // is suppressed entirely, see below). Placed first so it's
  // visible without scrolling on touch sheets.
  const sheetActions: CoverMenuAction[] =
    onEnterSelectMode && !selectMode?.isActive
      ? [
          {
            label: "Select",
            onSelect: () => onEnterSelectMode(issue.id),
          },
          ...menuActions,
        ]
      : menuActions;
  const longPress = useCoverLongPressActions({
    primary:
      issue.state === "active"
        ? {
            label: `Read ${heading}`,
            onSelect: () => router.push(readerUrl(issue)),
          }
        : undefined,
    actions: sheetActions,
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
  //
  // When `selectMode.isActive` is true the outer element is rendered
  // as a `<button>` and the long-press wrapper props are not spread
  // onto the cover — the gesture stays dormant so taps toggle
  // selection instead of opening the existing actions sheet.
  const inSelectMode = selectMode?.isActive ?? false;
  const cardOuterProps = inSelectMode
    ? {
        type: "button" as const,
        onClick: (ev: React.MouseEvent) => {
          ev.preventDefault();
          selectMode?.onToggle(ev);
        },
        "aria-pressed": selectMode?.isSelected ?? false,
      }
    : null;
  const coverWrapperProps = inSelectMode ? {} : longPress.wrapperProps;
  const cardClassName = cn(
    "group hover:bg-accent/40 focus-visible:ring-ring flex flex-col gap-2 rounded-md p-1 transition-colors focus-visible:ring-2 focus-visible:outline-none",
    inSelectMode && "text-left w-full cursor-pointer",
    inSelectMode &&
      selectMode?.isSelected &&
      "bg-primary/5 ring-2 ring-primary/40",
    className,
  );
  const innerCard = (
    <>
      <div className="relative" {...coverWrapperProps}>
        {selectMode && (
          <SelectionCheckbox
            isSelected={selectMode.isSelected}
            selectMode={selectMode.isActive}
            onToggle={selectMode.onToggle}
            label={heading}
          />
        )}
        <Cover
          src={issue.cover_url}
          alt={heading}
          fallback={numberLabel}
          className="w-full transition group-hover:brightness-110"
        />
        {/* Top-right priority cascade (the cover-card standard, see
         *  docs/dev/card-corner-conventions.md):
         *    1. match-status badge — CBL detail page only, doesn't apply here
         *    2. state badge       — when `state !== "active"` (archived/withdrawn)
         *    3. finished check    — when `state === "active" && finished`
         *  These three are mutually exclusive; only one ever renders.
         *  Bottom-left stays empty on this card (used to host the
         *  finished check; freed up for future indicators — downloaded
         *  badge, age rating, queue marker, etc.). */}
        {issue.state !== "active" && (
          <Badge
            variant="destructive"
            className="bg-background/80 absolute top-2 right-2 backdrop-blur"
          >
            {issue.state}
          </Badge>
        )}
        {issue.state === "active" && finished && (
          <span
            aria-label="Read"
            title="Read"
            className="bg-primary/90 text-primary-foreground absolute top-2 right-2 inline-flex h-6 w-6 items-center justify-center rounded-full shadow-sm ring-1 ring-black/10 backdrop-blur dark:ring-white/10"
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
        {issue.state === "active" && !inSelectMode && (
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
    </>
  );
  return (
    <>
      {cardOuterProps ? (
        <button className={cardClassName} {...cardOuterProps}>
          {innerCard}
        </button>
      ) : (
        <Link href={issueUrl(issue)} className={cardClassName}>
          {innerCard}
        </Link>
      )}
      {collectionActions.dialog}
      {!inSelectMode && longPress.sheet}
    </>
  );
}

/** Memoized: grid/rail surfaces mount hundreds of these at once and
 *  their props are referentially stable (cache rows + literals), so
 *  parent state churn — search keystrokes, selection toggles,
 *  sentinel observer resets — no longer reconciles every card. */
export const IssueCard = memo(IssueCardImpl);

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
