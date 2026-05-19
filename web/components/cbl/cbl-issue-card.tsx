"use client";

import { Check } from "lucide-react";
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
import { SelectionCheckbox } from "@/components/library/SelectionCheckbox";
import { Badge } from "@/components/ui/badge";
import { useUpsertIssueProgress } from "@/lib/api/mutations";
import { useUserProgress } from "@/lib/api/queries";
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
  selectMode,
  onEnterSelectMode,
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
  /** Multi-select mode (M6 extension). Mirrors `<IssueCard>`'s shape.
   *  Only meaningful when `issue` is set — placeholder cards (missing
   *  / ambiguous match) aren't selectable. */
  selectMode?: {
    isActive: boolean;
    isSelected: boolean;
    onToggle: (ev?: React.MouseEvent) => void;
  };
  /** Long-press sheet "Select" entry callback. */
  onEnterSelectMode?: (id: string) => void;
}) {
  const positionLabel = `#${entry.position + 1}`;
  const numberLabel = entry.issue_number ? `#${entry.issue_number}` : "—";
  const heading = issue?.title ?? entry.series_name;
  const router = useRouter();
  const upsertProgress = useUpsertIssueProgress();
  // Shared progress map — same hook IssueCard / SeriesCard read so the
  // read-check (finished) and percent bar (in progress) appear here
  // too. Map is keyed by issue id and dedupes via TanStack.
  const progress = useUserProgress().data?.get(issue?.id ?? "");
  const finished = progress?.finished ?? false;
  const inProgress = !!progress && !finished && progress.percent > 0;
  const percent = inProgress
    ? Math.max(0, Math.min(100, Math.round(progress.percent * 100)))
    : 0;
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
  const inSelectMode = selectMode?.isActive ?? false;
  const sheetActions: CoverMenuAction[] =
    onEnterSelectMode && !inSelectMode && issue
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
      issue && issue.state === "active"
        ? {
            label: `Read ${heading}`,
            onSelect: () =>
              router.push(readerUrl(issue, { cbl: cblSavedViewId })),
          }
        : undefined,
    actions: sheetActions,
    label: `${positionLabel} · ${heading}`,
  });

  // When in select mode, the long-press wrapper is suppressed and
  // taps toggle selection instead of navigating — same pattern as
  // `<IssueCard>` / `<SeriesCard>`.
  const coverWrapperProps = inSelectMode ? {} : longPress.wrapperProps;

  const cover = (showActions: boolean) => (
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
      {/* Read state — bottom-right to leave bottom-left for the CBL
       *  position badge. Mirrors the affordance on `IssueCard`
       *  (which has no position badge so places its check at
       *  bottom-left); shape + ring + size kept in lockstep so the
       *  visual cue is identical across surfaces. */}
      {issue && issue.state === "active" && finished && (
        <span
          aria-label="Read"
          title="Read"
          className="bg-primary/90 text-primary-foreground absolute right-2 bottom-2 inline-flex h-6 w-6 items-center justify-center rounded-full ring-1 shadow-sm ring-black/10 backdrop-blur dark:ring-white/10"
        >
          <Check aria-hidden="true" className="h-3.5 w-3.5" />
        </span>
      )}
      {issue && issue.state === "active" && inProgress && (
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
      {showActions && issue && !inSelectMode && (
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
    inSelectMode && "text-left w-full cursor-pointer",
    inSelectMode &&
      selectMode?.isSelected &&
      "bg-primary/5 ring-2 ring-primary/40",
    className,
  );
  // Dialog must render outside <Link> — see SeriesCard for the bubble
  // explanation. Wrap in a fragment so the dialog ends up as a Link
  // sibling rather than a child.
  if (issue) {
    return (
      <>
        {inSelectMode ? (
          <button
            type="button"
            onClick={(ev) => {
              ev.preventDefault();
              selectMode?.onToggle(ev);
            }}
            aria-pressed={selectMode?.isSelected ?? false}
            className={wrapClass}
          >
            {cover(issue.state === "active")}
            {meta}
          </button>
        ) : (
          <Link
            href={issueUrl(issue, { cbl: cblSavedViewId })}
            className={wrapClass}
          >
            {cover(issue.state === "active")}
            {meta}
          </Link>
        )}
        {collectionActions.dialog}
        {!inSelectMode && longPress.sheet}
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
