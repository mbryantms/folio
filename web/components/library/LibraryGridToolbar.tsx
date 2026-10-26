"use client";

import * as React from "react";
import { BookmarkPlus, Filter, X } from "lucide-react";

import { CardSizeOptions } from "@/components/library/CardSizeOptions";
import type { LibraryGridMode } from "@/components/library/library-grid-filters";
import { StickyToolbarRow } from "@/components/library/StickyToolbarRow";
import { SelectModeButton } from "@/components/library/SelectModeButton";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/folio/select";
import type { IssueSort, SeriesSort, SortOrder } from "@/lib/api/types";

// `random` is excluded — it's a one-shot discovery pick ("Surprise me"),
// never a user-selectable grid sort. The Exclude keeps this map exhaustive
// over the *pickable* sorts so adding a real sort still fails the build.
export const SERIES_SORT_LABELS: Record<
  Exclude<SeriesSort, "random">,
  string
> = {
  name: "Name",
  created_at: "Recently added",
  updated_at: "Recently updated",
  // Series only carry a year — say so (post-audit T2.3).
  year: "Year",
};

export const ISSUE_SORT_LABELS: Record<IssueSort, string> = {
  number: "Issue number",
  created_at: "Recently added",
  updated_at: "Recently updated",
  // Issues order by (year, month, day) server-side, so the label holds.
  year: "Release date",
  page_count: "Page count",
  user_rating: "My rating",
};

/** Series ⇄ Issues as a compact segmented control — the phone row's
 *  leading slot (`PhoneControlsBar`). Desktop keeps the two outline
 *  buttons below so the toolbar's h-9 band stays uniform. Carries
 *  `folio-control` so the touch-target rule (`globals.css`) covers it
 *  with its icon-button neighbours; the segments stretch to fill. */
export function ModeSegmentedToggle({
  mode,
  onMode,
}: {
  mode: LibraryGridMode;
  onMode: (m: LibraryGridMode) => void;
}) {
  return (
    <div
      role="group"
      aria-label="Browse series or issues"
      className="folio-control bg-muted inline-flex h-9 items-stretch gap-0.5 rounded-md p-0.5"
    >
      {(
        [
          { value: "series", label: "Series" },
          { value: "issues", label: "Issues" },
        ] as const
      ).map((o) => (
        <button
          key={o.value}
          type="button"
          aria-pressed={mode === o.value}
          onClick={() => onMode(o.value)}
          className={
            mode === o.value
              ? "folio-tap bg-background text-foreground rounded px-3 text-sm font-medium shadow-sm"
              : "folio-tap text-muted-foreground rounded px-3 text-sm"
          }
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}

/**
 * Desktop (`sm+`) toolbar above the library grid: mode toggle
 * (Series/Issues), search input, per-mode sort dropdown, order arrow,
 * Filters button, Save-as-view button, Clear-filters button, and the
 * card-size slider on the far right. Extracted from
 * `LibraryGridView.tsx` in audit-remediation M7.3 to keep the composer
 * under the 400-LOC target. Phones render `PhoneControlsBar` instead —
 * this row folded into six on a 390px viewport.
 *
 * Like the phone bar, the row sticks under the topbar — but only while
 * it fits on one line and select mode is off (`useStickyToolbar`): a
 * wrapped toolbar stays in flow rather than pinning three rows, and in
 * select mode the sticky `SelectionToolbar` takes the band instead,
 * exactly as the phone bar hides itself then. The negative horizontal
 * margins let the band run edge to edge inside the shell's `px-4` /
 * `md:px-8` main column; the border and backdrop only appear once the
 * row is actually held (`stuck`), so the page looks unchanged at rest.
 */
export function LibraryGridToolbar({
  mode,
  onMode,
  q,
  onQ,
  trimmedQ,
  seriesSort,
  onSeriesSort,
  issueSort,
  onIssueSort,
  order,
  onOrder,
  facetCount,
  onOpenFilters,
  canSaveView,
  onSaveView,
  onClearFacets,
  cardSize,
  onCardSize,
  cardSizeMin,
  cardSizeMax,
  cardSizeStep,
  cardSizeDefault,
  canSelect,
  selectMode,
  onEnterSelect,
  onExitSelect,
  selectButtonRef,
}: {
  mode: LibraryGridMode;
  onMode: (m: LibraryGridMode) => void;
  q: string;
  onQ: (q: string) => void;
  trimmedQ: string;
  seriesSort: SeriesSort;
  onSeriesSort: (s: SeriesSort) => void;
  issueSort: IssueSort;
  onIssueSort: (s: IssueSort) => void;
  order: SortOrder;
  onOrder: (o: SortOrder) => void;
  facetCount: number;
  onOpenFilters: () => void;
  canSaveView: boolean;
  onSaveView: () => void;
  onClearFacets: () => void;
  cardSize: number;
  onCardSize: (n: number) => void;
  cardSizeMin: number;
  cardSizeMax: number;
  cardSizeStep: number;
  cardSizeDefault: number;
  /** True once the grid has at least one loaded card to act on. */
  canSelect: boolean;
  /** Whether the grid is currently in multi-select mode. */
  selectMode: boolean;
  /** Enter multi-select mode (surfaces the SelectionToolbar). */
  onEnterSelect: () => void;
  /** Exit multi-select mode (the button toggles to Cancel while active). */
  onExitSelect: () => void;
  /** Forwarded so the grid can restore focus here on select-mode exit. */
  selectButtonRef?: React.Ref<HTMLButtonElement>;
}) {
  // The live input string is local to the toolbar; `q` from the
  // filters hook is the *debounced* value. Keeping the raw keystrokes
  // here means typing re-renders this small toolbar, not the whole
  // grid of mounted cards. `lastSent` distinguishes our own debounced
  // echo (ignore — clobbering the input would drop trailing spaces
  // mid-typing) from external changes like Clear filters (sync).
  const [searchInput, setSearchInput] = React.useState(q);
  const lastSent = React.useRef(q);
  React.useEffect(() => {
    if (q !== lastSent.current) {
      setSearchInput(q);
      lastSent.current = q;
    }
  }, [q]);
  return (
    <>
      <StickyToolbarRow
        enabled={!selectMode}
        data-library-toolbar=""
        className="mb-4 justify-between"
      >
        {/* Left controls flow + wrap as a group; the Select + card-size
          group below sits opposite via the container's justify-between —
          so it right-aligns when there's room and drops to a left-aligned
          new line (no stranded empty space) when the toolbar wraps. */}
        <div className="flex flex-wrap items-center gap-2">
          {/* Mode toggle: two side-by-side buttons that match the rest
          of the toolbar (same `size="toolbar"` + outline base; active mode
          takes `variant="secondary"` for visual contrast). Earlier
          iterations used a bordered wrapper around the pair, which
          ran ~2px taller than the peer Sort / Order / Filters
          buttons — this version sits flush. */}
          <Button
            type="button"
            variant={mode === "series" ? "secondary" : "outline"}

            aria-pressed={mode === "series"}
            onClick={() => onMode("series")}
          >
            Series
          </Button>
          <Button
            type="button"
            variant={mode === "issues" ? "secondary" : "outline"}

            aria-pressed={mode === "issues"}
            onClick={() => onMode("issues")}
          >
            Issues
          </Button>
          <Input
            type="search"
            placeholder={
              mode === "series" ? "Search series…" : "Search issues…"
            }
            value={searchInput}
            onChange={(e) => {
              const next = e.target.value;
              setSearchInput(next);
              lastSent.current = next.trim();
              onQ(next);
            }}
            className="h-9 w-72"
          />
          {mode === "series" ? (
            <Select
              value={seriesSort}
              onValueChange={(v) => onSeriesSort(v as SeriesSort)}
            >
              <SelectTrigger className="h-9 w-44" disabled={!!trimmedQ}>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {(
                  Object.keys(SERIES_SORT_LABELS) as Array<
                    keyof typeof SERIES_SORT_LABELS
                  >
                ).map((s) => (
                  <SelectItem key={s} value={s}>
                    {SERIES_SORT_LABELS[s]}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          ) : (
            <Select
              value={issueSort}
              onValueChange={(v) => onIssueSort(v as IssueSort)}
            >
              <SelectTrigger className="h-9 w-44" disabled={!!trimmedQ}>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {(Object.keys(ISSUE_SORT_LABELS) as IssueSort[]).map((s) => (
                  <SelectItem key={s} value={s}>
                    {ISSUE_SORT_LABELS[s]}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          )}
          <Button
            type="button"
            variant="outline"

            disabled={!!trimmedQ}
            onClick={() => onOrder(order === "asc" ? "desc" : "asc")}
            title={`Order: ${order === "asc" ? "Ascending" : "Descending"}`}
            className="w-9"
          >
            {order === "asc" ? "↑" : "↓"}
          </Button>

          <Button
            type="button"
            variant="outline"

            onClick={onOpenFilters}
          >
            <Filter className="mr-1 h-3.5 w-3.5" />
            Filters
            {facetCount > 0 ? (
              <Badge
                variant="secondary"
                className="ml-2 h-5 min-w-5 rounded-full px-1.5 text-xs"
              >
                {facetCount}
              </Badge>
            ) : null}
          </Button>

          <Button
            type="button"
            variant="outline"

            disabled={!canSaveView}
            onClick={onSaveView}
            title="Persist these filters as a new saved view"
          >
            <BookmarkPlus className="mr-1 h-3.5 w-3.5" />
            Save as view…
          </Button>

          {facetCount > 0 ? (
            <Button
              type="button"
              variant="ghost"

              onClick={onClearFacets}
              className="text-muted-foreground"
            >
              <X className="mr-1 h-3 w-3" /> Clear filters
            </Button>
          ) : null}
        </div>

        <div className="flex items-center gap-2">
          {canSelect || selectMode ? (
            <SelectModeButton
              ref={selectButtonRef}
              active={selectMode}
              onEnter={onEnterSelect}
              onExit={onExitSelect}
            />
          ) : null}
          <CardSizeOptions
            cardSize={cardSize}
            onCardSize={onCardSize}
            min={cardSizeMin}
            max={cardSizeMax}
            step={cardSizeStep}
            defaultSize={cardSizeDefault}
          />
        </div>
      </StickyToolbarRow>
    </>
  );
}
