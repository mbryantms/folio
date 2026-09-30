"use client";

import * as React from "react";
import { Sparkles } from "lucide-react";
import { useRouter } from "next/navigation";
import { toast } from "sonner";

import { CardSizeOptions } from "@/components/library/CardSizeOptions";
import { IssueCard, IssueCardSkeleton } from "@/components/library/IssueCard";
import { useCardSize } from "@/components/library/use-card-size";
import { DropdownMenuItem } from "@/components/ui/dropdown-menu";
import { EmptyState } from "@/components/ui/empty-state";
import { useCreateSavedViewBatch } from "@/lib/api/mutations";
import { useSavedViewIssueResultsInfinite } from "@/lib/api/queries";
import type { SavedViewView } from "@/lib/api/types";

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
 *  IntersectionObserver sentinel like `<FilterViewDetail>`. */
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
    gridTemplateColumns: `repeat(auto-fill, minmax(${cardSize}px, 1fr))`,
  };
  const items = (results.data?.pages ?? []).flatMap((p) => p.items);

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
            <Sparkles className="mr-2 h-4 w-4" />
            Fetch metadata for view
          </DropdownMenuItem>
        }
        extraActions={
          <CardSizeOptions
            cardSize={cardSize}
            onCardSize={setCardSize}
            min={CARD_SIZE_MIN}
            max={CARD_SIZE_MAX}
            step={CARD_SIZE_STEP}
            defaultSize={CARD_SIZE_DEFAULT}
          />
        }
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
              <IssueCard issue={issue} />
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
    </div>
  );
}
