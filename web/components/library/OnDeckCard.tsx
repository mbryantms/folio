"use client";

import Link from "next/link";
import { useRouter } from "next/navigation";

import { Cover } from "@/components/Cover";
import {
  CoverMenuButton,
  type CoverMenuAction,
} from "@/components/CoverMenuButton";
import { useCoverLongPressActions } from "@/components/CoverLongPressActions";
import { QuickReadOverlay } from "@/components/QuickReadOverlay";
import { formatIssueHeading } from "@/lib/format";
import { cn } from "@/lib/utils";
import type { OnDeckCard as OnDeckCardData } from "@/lib/api/types";
import { issueUrl, readerUrl } from "@/lib/urls";
import {
  useDismissRailItem,
  useUpsertIssueProgress,
} from "@/lib/api/mutations";

/**
 * On Deck rail card. Two flavors share the same visual shell:
 *
 *  - `series_next`: cover + "{series} · #N" label, primary text is the
 *    issue title; the card links to the reader.
 *  - `cbl_next`: cover + "{cbl name} · entry N" label; the card links to
 *    the reader for the next-unread matched entry.
 *
 * The kebab menu lets the user mark the issue read (advances the queue)
 * or hide the underlying series / CBL from the rail (auto-restores on
 * the next new-progress event).
 */
export function OnDeckCard({
  card,
  className,
}: {
  card: OnDeckCardData;
  className?: string;
}) {
  const dismiss = useDismissRailItem();
  const upsertProgress = useUpsertIssueProgress();
  const router = useRouter();
  const issue = card.issue;
  const numberLabel = issue.number ? `#${issue.number}` : "—";
  // `series_next` cards carry the series name as a sibling field;
  // `cbl_next` cards don't (the meta line shows the list name instead),
  // but the issue payload still has `series_name` populated server-side
  // for the heading fallback.
  const seriesName =
    card.kind === "series_next"
      ? card.series_name
      : (issue.series_name ?? null);
  const heading = formatIssueHeading(issue, seriesName);

  const meta =
    card.kind === "series_next"
      ? `${card.series_name} · ${numberLabel}`
      : `${card.cbl_list_name} · entry ${card.position}`;

  const removeAction =
    card.kind === "series_next"
      ? {
          label: "Hide series from rail",
          target_kind: "series" as const,
          target_id: card.issue.series_id,
        }
      : {
          label: "Hide list from rail",
          target_kind: "cbl" as const,
          target_id: card.cbl_list_id,
        };

  const menuActions: CoverMenuAction[] = [
    {
      label: "Mark this issue as read",
      onSelect: () => {
        const pageCount = issue.page_count ?? 1;
        upsertProgress.mutate({
          issue_id: issue.id,
          page: pageCount > 0 ? pageCount - 1 : 0,
          finished: true,
        });
      },
    },
    {
      label: "Mark this issue as unread",
      onSelect: () =>
        upsertProgress.mutate({
          issue_id: issue.id,
          page: 0,
          finished: false,
        }),
    },
    {
      label: removeAction.label,
      onSelect: () =>
        dismiss.mutate({
          target_kind: removeAction.target_kind,
          target_id: removeAction.target_id,
        }),
    },
  ];
  const longPress = useCoverLongPressActions({
    primary: {
      label: `Read ${heading}`,
      onSelect: () => router.push(readerUrl(issue)),
    },
    actions: menuActions,
    label: meta,
  });

  // Cover/title click → issue detail page (matches every other cover-
  // bearing card). The yellow play overlay is the ONLY surface that
  // routes straight to the reader.
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
          <CoverMenuButton
            label={`Actions for ${heading}`}
            actions={menuActions}
          />
          <QuickReadOverlay
            readerHref={readerUrl(issue)}
            label={`Read ${heading}`}
          />
        </div>
        <div className="min-w-0 px-1">
          <div className="text-muted-foreground truncate text-xs" title={meta}>
            {meta}
          </div>
          <div className="truncate text-sm font-medium" title={heading}>
            {heading}
          </div>
          <div className="text-muted-foreground mt-0.5 text-[11px]">
            Up next
          </div>
        </div>
      </Link>
      {longPress.sheet}
    </>
  );
}

export function OnDeckCardSkeleton({ className }: { className?: string }) {
  return (
    <div className={cn("flex flex-col gap-2 p-1", className)}>
      <div className="bg-muted aspect-[2/3] w-full animate-pulse rounded-md" />
      <div className="space-y-1.5 px-1">
        <div className="bg-muted h-2 w-1/2 animate-pulse rounded" />
        <div className="bg-muted h-3 w-3/4 animate-pulse rounded" />
        <div className="bg-muted h-2 w-1/4 animate-pulse rounded" />
      </div>
    </div>
  );
}
