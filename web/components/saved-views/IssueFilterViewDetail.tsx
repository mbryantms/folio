"use client";

import * as React from "react";
import {
  BookOpen,
  BookOpenCheck,
  FolderPlus,
  ListChecks,
  Sparkles,
} from "lucide-react";
import { useRouter } from "next/navigation";
import { toast } from "sonner";

import { BulkAddToCollectionDialog } from "@/components/collections/BulkAddToCollectionDialog";
import {
  BulkMarkReadDialog,
  BULK_BACKFILL_PROMPT_THRESHOLD,
} from "@/components/library/BulkMarkReadDialog";
import {
  CardSizeFields,
  CardSizeOptions,
} from "@/components/library/CardSizeOptions";
import {
  PhoneControlsBar,
  PhoneMenuAction,
} from "@/components/library/PhoneControlsBar";
import { IssueCard, IssueCardSkeleton } from "@/components/library/IssueCard";
import { SelectModeButton } from "@/components/library/SelectModeButton";
import { SelectionToolbar } from "@/components/library/SelectionToolbar";
import { useCardSize } from "@/components/library/use-card-size";
import { DropdownMenuItem } from "@/components/folio/dropdown-menu";
import { EmptyState } from "@/components/folio/empty-state";
import {
  useBulkMarkProgress,
  useCreateSavedViewBatch,
} from "@/lib/api/mutations";
import { useSavedViewIssueResultsInfinite } from "@/lib/api/queries";
import type { SavedViewView } from "@/lib/api/types";
import { shouldSkipHotkey } from "@/lib/reader/keybinds";
import { useSelection } from "@/lib/selection/use-selection";

import { EditFilterViewSheet } from "./EditFilterViewSheet";
import { ViewHeader } from "./ViewHeader";

const CARD_SIZE_MIN = 120;
const CARD_SIZE_MAX = 280;
const CARD_SIZE_STEP = 20;
const CARD_SIZE_DEFAULT = 160;
/** Shared with `<FilterViewDetail>` so one density slider covers every
 *  filter-view page. */
const CARD_SIZE_STORAGE_KEY = "folio.savedView.cardSize";

/** Issue-level (`filter_issues`, WP-5.4) view detail page: header +
 *  cursor-paginated issue grid, auto-loading the next page from an
 *  IntersectionObserver sentinel like `<FilterViewDetail>`.
 *
 *  Multi-select (audit 2026-11 N-O4): the same `useSelection` +
 *  `SelectionToolbar` wiring as the series twin, with the issue-shaped
 *  bulk actions the library grid's issues mode offers — Mark read /
 *  Mark unread (`/me/progress/bulk`, backfill prompt at the shared
 *  threshold) and Add to collection. Only the loaded pages are
 *  selectable, as on every other saved-view surface. */
export function IssueFilterViewDetail({ view }: { view: SavedViewView }) {
  const [editOpen, setEditOpen] = React.useState(false);
  const results = useSavedViewIssueResultsInfinite(view.id);
  const router = useRouter();
  const createBatch = useCreateSavedViewBatch();

  const sentinelRef = React.useRef<HTMLDivElement | null>(null);
  const { hasNextPage, isFetchingNextPage, fetchNextPage } = results;
  React.useEffect(() => {
    const el = sentinelRef.current;
    if (!el) return;
    const obs = new IntersectionObserver(
      (entries) => {
        if (entries.some((e) => e.isIntersecting)) {
          if (hasNextPage && !isFetchingNextPage) {
            void fetchNextPage();
          }
        }
      },
      { rootMargin: "600px" },
    );
    obs.observe(el);
    return () => obs.disconnect();
  }, [hasNextPage, isFetchingNextPage, fetchNextPage]);

  const fetchViewMetadata = () => {
    createBatch.mutate(
      { saved_view_id: view.id },
      {
        onSuccess: (resp) => {
          if (!resp) return;
          toast.success(`Searching ${resp.items_total} items for metadata`, {
            action: {
              label: "Review",
              onClick: () =>
                router.push(
                  `/admin/metadata?tab=review&batch=${resp.batch_id}`,
                ),
            },
          });
        },
      },
    );
  };

  const [cardSize, setCardSize] = useCardSize({
    storageKey: CARD_SIZE_STORAGE_KEY,
    min: CARD_SIZE_MIN,
    max: CARD_SIZE_MAX,
    defaultSize: CARD_SIZE_DEFAULT,
  });
  const gridStyle: React.CSSProperties = {
    gridTemplateColumns: `repeat(auto-fill, minmax(min(${cardSize}px, calc(50% - 0.5rem)), 1fr))`,
  };
  const items = React.useMemo(
    () => (results.data?.pages ?? []).flatMap((p) => p.items),
    [results.data],
  );

  // Multi-select over the loaded issues. Mark read at scale prompts for
  // the reading-log backfill flag at the same threshold as the library
  // grid; mark unread never prompts.
  const selection = useSelection(items);
  const bulkMark = useBulkMarkProgress();
  const [pickerOpen, setPickerOpen] = React.useState(false);
  const [markReadOpen, setMarkReadOpen] = React.useState(false);
  const submitMarkRead = (backfill: boolean) => {
    const issue_ids = Array.from(selection.selected);
    if (issue_ids.length === 0) return;
    bulkMark.mutate(
      { issue_ids, finished: true, backfill },
      {
        onSuccess: () => {
          selection.exit();
          setMarkReadOpen(false);
        },
      },
    );
  };
  const runBulk = (finished: boolean) => {
    const issue_ids = Array.from(selection.selected);
    if (issue_ids.length === 0) return;
    if (finished && issue_ids.length >= BULK_BACKFILL_PROMPT_THRESHOLD) {
      setMarkReadOpen(true);
      return;
    }
    bulkMark.mutate(
      { issue_ids, finished },
      { onSuccess: () => selection.exit() },
    );
  };
  const selectedTargets = Array.from(selection.selected).map((id) => ({
    entry_kind: "issue" as const,
    ref_id: id,
  }));

  const selectButtonRef = React.useRef<HTMLButtonElement | null>(null);
  const wasSelectModeRef = React.useRef(false);
  // Esc exits select mode; Cmd/Ctrl+A selects every loaded card.
  // `shouldSkipHotkey` keeps the bindings dormant while focus is in
  // a form field. The hook's callbacks are stable, so the listener is
  // bound once per select-mode session rather than per toggle.
  const { selectMode, exit: exitSelection, selectAll } = selection;
  React.useEffect(() => {
    if (!selectMode) return;
    const onKey = (e: KeyboardEvent) => {
      if (shouldSkipHotkey(e)) return;
      if (e.key === "Escape") {
        e.preventDefault();
        exitSelection();
      } else if (e.key === "a" && (e.metaKey || e.ctrlKey)) {
        e.preventDefault();
        selectAll();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [selectMode, exitSelection, selectAll]);
  // Restore focus to the Select trigger after leaving select mode.
  React.useEffect(() => {
    if (wasSelectModeRef.current && !selection.selectMode) {
      selectButtonRef.current?.focus();
    }
    wasSelectModeRef.current = selection.selectMode;
  }, [selection.selectMode]);

  return (
    <div className="space-y-6">
      <ViewHeader
        view={view}
        onEdit={() => setEditOpen(true)}
        extraMenuItems={
          <DropdownMenuItem
            onSelect={fetchViewMetadata}
            disabled={createBatch.isPending}
          >
            <Sparkles className="mr-2 size-4" />
            Fetch metadata for view
          </DropdownMenuItem>
        }
        // phone-layout rule 11: [Select] [▦]; the header appends [⋯].
        extraActions={
          <>
            {(items.length > 0 || selection.selectMode) && (
              <SelectModeButton
                ref={selectButtonRef}
                active={selection.selectMode}
                onEnter={() => selection.enter()}
                onExit={() => selection.exit()}
              />
            )}
            <CardSizeOptions
              cardSize={cardSize}
              onCardSize={setCardSize}
              min={CARD_SIZE_MIN}
              max={CARD_SIZE_MAX}
              step={CARD_SIZE_STEP}
              defaultSize={CARD_SIZE_DEFAULT}
            />
          </>
        }
        // Phone (< sm): the desktop cluster above is hidden; card size
        // and Select move into the sticky bar's View options sheet. The
        // bar yields to `SelectionToolbar` (also sticky) while selecting.
        phoneControlsPlacement="inline"
        phoneControls={
          <PhoneControlsBar
            sticky={false}
            hidden={selection.selectMode}
            menu={(close) => (
              <>
                <CardSizeFields
                  cardSize={cardSize}
                  onCardSize={setCardSize}
                  min={CARD_SIZE_MIN}
                  max={CARD_SIZE_MAX}
                  step={CARD_SIZE_STEP}
                  defaultSize={CARD_SIZE_DEFAULT}
                  fieldId="phone-card-size"
                />
                {items.length > 0 ? (
                  <PhoneMenuAction
                    icon={<ListChecks className="size-4" />}
                    onClick={() => {
                      close();
                      selection.enter();
                    }}
                  >
                    Select
                  </PhoneMenuAction>
                ) : null}
              </>
            )}
          />
        }
      />

      <SelectionToolbar
        open={selection.selectMode}
        count={selection.count}
        total={items.length}
        primary={[
          {
            id: "mark-read",
            label: "Mark read",
            icon: BookOpenCheck,
            onClick: () => runBulk(true),
            disabled: bulkMark.isPending || selection.count === 0,
          },
          {
            id: "mark-unread",
            label: "Mark unread",
            icon: BookOpen,
            onClick: () => runBulk(false),
            disabled: bulkMark.isPending || selection.count === 0,
          },
        ]}
        overflow={[
          {
            id: "add-to-collection",
            label: "Add to collection…",
            icon: FolderPlus,
            onClick: () => setPickerOpen(true),
            disabled: selection.count === 0,
          },
        ]}
        onDone={() => selection.exit()}
        onClear={() => selection.clear()}
        onSelectAll={() => selection.selectAll()}
        isPending={bulkMark.isPending}
      />

      {results.isLoading ? (
        <ul role="list" className="grid gap-4" style={gridStyle}>
          {Array.from({ length: 12 }).map((_, i) => (
            <li key={i}>
              <IssueCardSkeleton />
            </li>
          ))}
        </ul>
      ) : items.length === 0 ? (
        <EmptyState
          size="sm"
          description="No issues match this view yet. Tweak the conditions to broaden the search."
        />
      ) : (
        <ul role="list" className="grid gap-4" style={gridStyle}>
          {items.map((issue) => (
            <li key={issue.id}>
              <IssueCard
                issue={issue}
                selectMode={
                  selection.selectMode
                    ? {
                        isActive: true,
                        isSelected: selection.isSelected(issue.id),
                        onToggle: (ev) => selection.toggle(issue.id, ev),
                      }
                    : undefined
                }
                onEnterSelectMode={(id) => selection.toggle(id)}
              />
            </li>
          ))}
        </ul>
      )}

      <div
        ref={sentinelRef}
        aria-hidden="true"
        className={results.hasNextPage ? "h-12" : "hidden"}
      />
      {results.isFetchingNextPage ? (
        <p className="text-muted-foreground text-center text-xs">
          Loading more…
        </p>
      ) : null}

      <EditFilterViewSheet
        view={view}
        open={editOpen}
        onOpenChange={setEditOpen}
      />

      <BulkAddToCollectionDialog
        open={pickerOpen}
        onOpenChange={(next) => {
          setPickerOpen(next);
          if (!next) selection.clear();
        }}
        targets={selectedTargets}
      />
      <BulkMarkReadDialog
        open={markReadOpen}
        onOpenChange={setMarkReadOpen}
        count={selection.count}
        onConfirm={submitMarkRead}
        isPending={bulkMark.isPending}
      />
    </div>
  );
}
