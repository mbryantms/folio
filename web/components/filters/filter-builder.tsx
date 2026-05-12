"use client";

import * as React from "react";
import { Plus } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Separator } from "@/components/ui/separator";
import {
  SeriesCard,
  SeriesCardSkeleton,
} from "@/components/library/SeriesCard";
import { previewSavedView } from "@/lib/api/queries";
import type {
  Condition,
  FilterDsl,
  MatchMode,
  PreviewReq,
  SavedViewSortField,
  SeriesListView,
  SortOrder,
} from "@/lib/api/types";
import { HttpError } from "@/lib/api/queries";

import { ConditionRow } from "./condition-row";
import { specFor } from "./field-registry";

export type FilterBuilderState = {
  name: string;
  description: string;
  matchMode: MatchMode;
  conditions: Condition[];
  sortField: SavedViewSortField;
  sortOrder: SortOrder;
  resultLimit: number;
};

export type FilterBuilderProps = {
  initial?: Partial<FilterBuilderState>;
  /** When provided, the Save button stays visible. The builder remains
   *  fully usable when omitted (preview-only mode). */
  onSave?: (state: FilterBuilderState) => Promise<void> | void;
  onCancel?: () => void;
  /** Optional library scope passed to async option lookups. */
  library?: string;
  saveLabel?: string;
};

const DEFAULT_STATE: FilterBuilderState = {
  name: "",
  description: "",
  matchMode: "all",
  conditions: [],
  sortField: "created_at",
  sortOrder: "desc",
  resultLimit: 12,
};

// Labels mirror the grid sort dropdowns (LibraryGridView's
// `SERIES_SORT_LABELS` / `ISSUE_SORT_LABELS`) so users see the same
// names in saved views, the series list, and the issues list. The two
// per-user axes (`last_read`, `read_progress`) are exclusive to saved
// views today — extending the grid sort enums to expose them would
// need a server-side enum + cursor change; tracked as a follow-up.
const SORT_FIELDS: { value: SavedViewSortField; label: string }[] = [
  { value: "name", label: "Name" },
  { value: "year", label: "Release date" },
  { value: "created_at", label: "Recently added" },
  { value: "updated_at", label: "Recently updated" },
  { value: "last_read", label: "Last read" },
  { value: "read_progress", label: "Read progress" },
];

function mergeInitial(
  initial?: Partial<FilterBuilderState>,
): FilterBuilderState {
  return { ...DEFAULT_STATE, ...(initial ?? {}) };
}

export function FilterBuilder({
  initial,
  onSave,
  onCancel,
  library,
  saveLabel = "Save view",
}: FilterBuilderProps) {
  const [state, setState] = React.useState<FilterBuilderState>(() =>
    mergeInitial(initial),
  );
  const [preview, setPreview] = React.useState<SeriesListView | null>(null);
  const [previewError, setPreviewError] = React.useState<string | null>(null);
  const [isPreviewing, setIsPreviewing] = React.useState(false);
  const [isSaving, setIsSaving] = React.useState(false);

  function patch(p: Partial<FilterBuilderState>) {
    setState((prev) => ({ ...prev, ...p }));
  }

  function addCondition() {
    const seed = specFor("name");
    const next: Condition = { field: "name", op: seed.allowedOps[0] };
    patch({ conditions: [...state.conditions, next] });
  }

  function updateCondition(idx: number, next: Condition) {
    patch({
      conditions: state.conditions.map((c, i) => (i === idx ? next : c)),
    });
  }

  function removeCondition(idx: number) {
    patch({
      conditions: state.conditions.filter((_, i) => i !== idx),
    });
  }

  async function runPreview() {
    setIsPreviewing(true);
    setPreviewError(null);
    try {
      const filter: FilterDsl = {
        match_mode: state.matchMode,
        conditions: state.conditions,
      };
      const req: PreviewReq = {
        filter,
        sort_field: state.sortField,
        sort_order: state.sortOrder,
        result_limit: state.resultLimit,
      };
      const res = await previewSavedView(req);
      setPreview(res);
    } catch (e) {
      const msg =
        e instanceof HttpError
          ? e.message
          : e instanceof Error
            ? e.message
            : "Preview failed";
      setPreviewError(msg);
    } finally {
      setIsPreviewing(false);
    }
  }

  async function handleSave() {
    if (!onSave) return;
    setIsSaving(true);
    try {
      await onSave(state);
    } finally {
      setIsSaving(false);
    }
  }

  function reset() {
    setState(mergeInitial(initial));
    setPreview(null);
    setPreviewError(null);
  }

  return (
    <div className="flex flex-col gap-4">
      <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
        <div className="flex flex-col gap-1">
          <Label htmlFor="filter-name">Name</Label>
          <Input
            id="filter-name"
            value={state.name}
            onChange={(e) => patch({ name: e.target.value })}
            placeholder="My horror shelf"
          />
        </div>
        <div className="flex flex-col gap-1">
          <Label htmlFor="filter-desc">Description (optional)</Label>
          <Input
            id="filter-desc"
            value={state.description}
            onChange={(e) => patch({ description: e.target.value })}
            placeholder="Why this view exists"
          />
        </div>
      </div>

      <Separator />

      <div className="flex flex-col gap-2">
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-2">
            <span className="text-muted-foreground text-sm">Match</span>
            <Select
              value={state.matchMode}
              onValueChange={(v) => patch({ matchMode: v as MatchMode })}
            >
              <SelectTrigger className="w-32">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="all">all of</SelectItem>
                <SelectItem value="any">any of</SelectItem>
              </SelectContent>
            </Select>
            <span className="text-muted-foreground text-sm">the following</span>
          </div>
          <Button
            type="button"
            variant="outline"
            size="sm"
            onClick={addCondition}
          >
            <Plus className="mr-1 h-4 w-4" /> Add condition
          </Button>
        </div>
        {state.conditions.length === 0 ? (
          <div className="text-muted-foreground rounded-md border border-dashed p-4 text-sm">
            No conditions yet. Add one to start filtering.
          </div>
        ) : (
          state.conditions.map((c, i) => (
            <ConditionRow
              key={i}
              condition={c}
              library={library}
              onChange={(next) => updateCondition(i, next)}
              onRemove={() => removeCondition(i)}
            />
          ))
        )}
      </div>

      <Separator />

      <div className="grid grid-cols-1 gap-3 sm:grid-cols-3">
        <div className="flex flex-col gap-1">
          <Label htmlFor="filter-sort-field">Sort by</Label>
          <Select
            value={state.sortField}
            onValueChange={(v) => patch({ sortField: v as SavedViewSortField })}
          >
            <SelectTrigger id="filter-sort-field">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {SORT_FIELDS.map((f) => (
                <SelectItem key={f.value} value={f.value}>
                  {f.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
        <div className="flex flex-col gap-1">
          <Label htmlFor="filter-sort-order">Order</Label>
          <Select
            value={state.sortOrder}
            onValueChange={(v) => patch({ sortOrder: v as SortOrder })}
          >
            <SelectTrigger id="filter-sort-order">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="desc">Descending</SelectItem>
              <SelectItem value="asc">Ascending</SelectItem>
            </SelectContent>
          </Select>
        </div>
        <div className="flex flex-col gap-1">
          <Label htmlFor="filter-limit">Limit</Label>
          <Input
            id="filter-limit"
            type="number"
            min={1}
            max={200}
            value={state.resultLimit}
            onChange={(e) => {
              const n = parseInt(e.target.value, 10);
              if (Number.isFinite(n))
                patch({ resultLimit: Math.max(1, Math.min(200, n)) });
            }}
          />
        </div>
      </div>

      <div className="flex flex-wrap items-center gap-2">
        <Button
          type="button"
          onClick={runPreview}
          disabled={isPreviewing || state.conditions.length === 0}
        >
          {isPreviewing ? "Previewing…" : "Preview"}
        </Button>
        {onSave ? (
          <Button
            type="button"
            variant="default"
            onClick={handleSave}
            disabled={
              isSaving ||
              state.name.trim() === "" ||
              state.conditions.length === 0
            }
          >
            {isSaving ? "Saving…" : saveLabel}
          </Button>
        ) : null}
        <Button type="button" variant="ghost" onClick={reset}>
          Reset
        </Button>
        {onCancel ? (
          <Button type="button" variant="ghost" onClick={onCancel}>
            Cancel
          </Button>
        ) : null}
      </div>

      {previewError ? (
        <div className="border-destructive bg-destructive/10 text-destructive rounded-md border p-3 text-sm">
          {previewError}
        </div>
      ) : null}

      <PreviewGrid preview={preview} loading={isPreviewing} />
    </div>
  );
}

function PreviewGrid({
  preview,
  loading,
}: {
  preview: SeriesListView | null;
  loading: boolean;
}) {
  // Use SeriesCard `size="md"` (`w-full`) inside the grid so each card
  // fills its column. The "sm" size has a fixed pixel width meant for
  // horizontal rails and overflows narrow grid columns.
  // Column count is tuned for the dialog width (max-w-4xl): four
  // columns max so cards stay legible. The wrapping scroll-container
  // (`overflow-y-auto` on the dialog body) handles tall result sets.
  const gridClass = "grid grid-cols-2 gap-3 sm:grid-cols-3 md:grid-cols-4";
  if (loading) {
    return (
      <div className={gridClass}>
        {Array.from({ length: 6 }).map((_, i) => (
          <SeriesCardSkeleton key={i} size="md" />
        ))}
      </div>
    );
  }
  if (!preview) return null;
  if (preview.items.length === 0) {
    return (
      <div className="text-muted-foreground rounded-md border border-dashed p-6 text-center text-sm">
        No matches.
      </div>
    );
  }
  return (
    <div className={gridClass}>
      {preview.items.map((s) => (
        <SeriesCard key={s.id} series={s} size="md" />
      ))}
    </div>
  );
}
