"use client";

import * as React from "react";
import { BookmarkPlus, Filter, MoreHorizontal, X } from "lucide-react";

import { CardSizeOptions } from "@/components/library/CardSizeOptions";
import type { LibraryGridMode } from "@/components/library/library-grid-filters";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import type { IssueSort, SeriesSort, SortOrder } from "@/lib/api/types";

const SERIES_SORT_LABELS: Record<SeriesSort, string> = {
  name: "Name",
  created_at: "Recently added",
  updated_at: "Recently updated",
  year: "Release date",
};

const ISSUE_SORT_LABELS: Record<IssueSort, string> = {
  number: "Issue number",
  created_at: "Recently added",
  updated_at: "Recently updated",
  year: "Release date",
  page_count: "Time to read",
  user_rating: "My rating",
};

/**
 * Toolbar above the library grid: mode toggle (Series/Issues), search
 * input, per-mode sort dropdown, order arrow, Filters button, Save-as-
 * view button, Clear-filters button, and the card-size slider on the
 * far right. Extracted from `LibraryGridView.tsx` in audit-remediation
 * M7.3 to keep the composer under the 400-LOC target.
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
}) {
  return (
    <div className="mb-6 flex flex-wrap items-center gap-2">
      {/* Mode toggle: two side-by-side buttons that match the rest
          of the toolbar (same `size="sm"` + outline base; active mode
          takes `variant="secondary"` for visual contrast). Earlier
          iterations used a bordered wrapper around the pair, which
          ran ~2px taller than the peer Sort / Order / Filters
          buttons — this version sits flush. */}
      <Button
        type="button"
        variant={mode === "series" ? "secondary" : "outline"}
        size="sm"
        aria-pressed={mode === "series"}
        onClick={() => onMode("series")}
        className="h-9"
      >
        Series
      </Button>
      <Button
        type="button"
        variant={mode === "issues" ? "secondary" : "outline"}
        size="sm"
        aria-pressed={mode === "issues"}
        onClick={() => onMode("issues")}
        className="h-9"
      >
        Issues
      </Button>
      <Input
        type="search"
        placeholder={mode === "series" ? "Search series…" : "Search issues…"}
        value={q}
        onChange={(e) => onQ(e.target.value)}
        // Grow to fill the row (caps via the flex container) instead of a
        // fixed width, so the toolbar collapses to fewer rows on mobile.
        className="h-9 min-w-0 flex-1 basis-48"
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
            {(Object.keys(SERIES_SORT_LABELS) as SeriesSort[]).map((s) => (
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
        size="sm"
        disabled={!!trimmedQ}
        onClick={() => onOrder(order === "asc" ? "desc" : "asc")}
        title={`Order: ${order === "asc" ? "Ascending" : "Descending"}`}
        className="h-9 w-9"
      >
        {order === "asc" ? "↑" : "↓"}
      </Button>

      <Button
        type="button"
        variant="outline"
        size="sm"
        onClick={onOpenFilters}
        className="h-9"
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

      <CardSizeOptions
        cardSize={cardSize}
        onCardSize={onCardSize}
        min={cardSizeMin}
        max={cardSizeMax}
        step={cardSizeStep}
        defaultSize={cardSizeDefault}
      />

      {/* Secondary actions collapse into an overflow so the toolbar stays
          short on mobile (they used to add up to two more wrapped rows). */}
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button
            type="button"
            variant="outline"
            size="sm"
            className="h-9 w-9"
            aria-label="More actions"
            title="More actions"
          >
            <MoreHorizontal className="h-4 w-4" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="min-w-[10rem]">
          <DropdownMenuItem
            disabled={!canSaveView}
            onSelect={(e) => {
              e.preventDefault();
              onSaveView();
            }}
          >
            <BookmarkPlus className="mr-2 h-4 w-4" />
            Save as view…
          </DropdownMenuItem>
          {facetCount > 0 ? (
            <DropdownMenuItem
              onSelect={(e) => {
                e.preventDefault();
                onClearFacets();
              }}
            >
              <X className="mr-2 h-4 w-4" />
              Clear filters
            </DropdownMenuItem>
          ) : null}
        </DropdownMenuContent>
      </DropdownMenu>
    </div>
  );
}
