"use client";

/**
 * `<MetadataCompareView>` — multi-provider collaborative merge.
 *
 * Tabular comparison: each included CANDIDATE is a COLUMN, fields are
 * ROWS. Columns are keyed by candidate `ordinal` (unique within a run),
 * so two candidates from the same provider are still distinct columns.
 * Each row is a radio group — "Keep mine" plus one cell per candidate
 * showing its proposed value. The default selection is the server's
 * merge-policy choice. The candidate header cards (cover + confidence)
 * double as remove toggles so the operator can drop a column and verify
 * each refers to the same issue before merging. One "Apply merged"
 * action sends the per-field candidate map to the composite-apply
 * endpoint.
 */

import { ArrowLeft, Loader2, X } from "lucide-react";

import { Button } from "@/components/ui/button";
import type { CompositeDiffResp } from "@/lib/api/types";

const KEEP_MINE = "__keep__";

function providerLabel(source: string): string {
  switch (source) {
    case "comicvine":
      return "ComicVine";
    case "metron":
      return "Metron";
    default:
      return source;
  }
}

export function defaultFieldSources(
  diff: CompositeDiffResp,
): Record<string, number> {
  const out: Record<string, number> = {};
  for (const row of diff.rows) {
    // Only pre-select fields that would actually change.
    if (
      row.chosen_ordinal != null &&
      (row.decision === "would_fill" || row.decision === "would_replace")
    ) {
      out[row.field] = row.chosen_ordinal;
    }
  }
  return out;
}

export function MetadataCompareView({
  data,
  isLoading,
  errorMessage,
  fieldSources,
  onRemoveColumn,
  onChangeFieldSource,
  onBack,
  onApply,
  isApplying,
}: {
  data: CompositeDiffResp | undefined;
  isLoading: boolean;
  errorMessage: string | null;
  fieldSources: Record<string, number>;
  onRemoveColumn: (ordinal: number) => void;
  onChangeFieldSource: (field: string, ordinal: number | null) => void;
  onApply: () => void;
  onBack: () => void;
  isApplying: boolean;
}) {
  if (isLoading) {
    return (
      <div className="text-muted-foreground flex items-center justify-center gap-2 py-12 text-sm">
        <Loader2 className="h-4 w-4 animate-spin" /> Comparing candidates…
      </div>
    );
  }
  if (errorMessage) {
    return (
      <div className="space-y-3 py-4">
        <p className="text-destructive text-sm">{errorMessage}</p>
        <Button variant="outline" size="sm" onClick={onBack}>
          <ArrowLeft className="mr-1.5 h-4 w-4" /> Back to matches
        </Button>
      </div>
    );
  }
  if (!data) return null;

  const columns = data.providers;
  const selectedCount = Object.keys(fieldSources).length;

  // [field label] [keep / current] [one per included candidate]
  const gridCols = `minmax(6.5rem, 1.1fr) minmax(6rem, 1fr) ${columns
    .map(() => "minmax(7rem, 1.4fr)")
    .join(" ")}`;

  return (
    <div className="space-y-3">
      <div className="flex items-center justify-between gap-2">
        <Button
          variant="ghost"
          size="sm"
          onClick={onBack}
          disabled={isApplying}
        >
          <ArrowLeft className="mr-1.5 h-3.5 w-3.5" /> Back to matches
        </Button>
        <span className="text-muted-foreground text-xs">
          Pick the best value per field; remove a candidate to drop its column.
        </span>
      </div>

      {/* Candidate header cards — verify each is the same issue + remove. */}
      <div className="flex flex-wrap gap-2">
        {columns.map((p) => (
          <div
            key={p.ordinal}
            className="border-ring/60 bg-card flex w-44 items-start gap-2 rounded-md border p-2 text-left"
          >
            {p.cover_image_url ? (
              // eslint-disable-next-line @next/next/no-img-element
              <img
                src={p.cover_image_url}
                alt={p.title ?? p.source}
                loading="lazy"
                className="h-16 w-11 flex-none rounded object-cover"
              />
            ) : (
              <div
                className="bg-muted h-16 w-11 flex-none rounded"
                aria-hidden
              />
            )}
            <div className="min-w-0 flex-1">
              <div className="flex items-center gap-1 text-xs font-medium">
                {providerLabel(p.source)}
                <span className="text-muted-foreground text-[10px] uppercase">
                  {p.bucket}
                </span>
              </div>
              <p className="text-muted-foreground truncate text-xs">
                {p.title ?? p.external_id}
              </p>
            </div>
            <button
              type="button"
              onClick={() => onRemoveColumn(p.ordinal)}
              disabled={isApplying || columns.length <= 1}
              className="text-muted-foreground hover:text-foreground flex-none disabled:opacity-30"
              title="Remove this candidate from the comparison"
              aria-label={`Remove ${providerLabel(p.source)}`}
            >
              <X className="h-3.5 w-3.5" />
            </button>
          </div>
        ))}
      </div>

      {columns.length === 0 ? (
        <p className="text-muted-foreground py-6 text-center text-sm">
          No candidates selected — go back and pick at least one to merge.
        </p>
      ) : (
        <div className="border-border max-h-[52vh] overflow-y-auto rounded-md border">
          {/* Sticky column header. */}
          <div
            className="bg-muted/60 border-border text-muted-foreground sticky top-0 z-10 grid items-end gap-x-2 border-b px-2 py-1.5 text-[11px] font-medium backdrop-blur"
            style={{ gridTemplateColumns: gridCols }}
          >
            <span>Field</span>
            <span>Keep mine</span>
            {columns.map((p) => (
              <span
                key={p.ordinal}
                className="truncate"
                title={p.title ?? undefined}
              >
                {providerLabel(p.source)}
                {p.title ? (
                  <span className="text-muted-foreground/70 ml-1 font-normal">
                    {p.title}
                  </span>
                ) : null}
              </span>
            ))}
          </div>

          {data.rows.map((row) => {
            const selected =
              row.field in fieldSources
                ? String(fieldSources[row.field])
                : KEEP_MINE;
            const cellClass = (active: boolean) =>
              `min-w-0 rounded px-1.5 py-1 ${active ? "bg-primary/10 ring-primary/40 ring-1" : ""}`;
            return (
              <div
                key={row.field}
                className="border-border/60 grid items-center gap-x-2 border-t px-2 py-1.5 text-sm"
                style={{ gridTemplateColumns: gridCols }}
              >
                <div className="min-w-0">
                  <div className="truncate font-medium">{row.label}</div>
                  {row.current_set_by && (
                    <div className="text-muted-foreground truncate text-[10px]">
                      set by {row.current_set_by}
                    </div>
                  )}
                </div>

                {/* Keep mine / current value. */}
                <label
                  className={`flex cursor-pointer items-center gap-1 ${cellClass(
                    selected === KEEP_MINE,
                  )}`}
                  title={row.current_value ?? "no current value"}
                >
                  <input
                    type="radio"
                    name={`fs-${row.field}`}
                    className="flex-none"
                    checked={selected === KEEP_MINE}
                    onChange={() => onChangeFieldSource(row.field, null)}
                  />
                  <span className="text-muted-foreground truncate text-xs">
                    {row.current_value ?? "—"}
                  </span>
                </label>

                {/* One cell per included candidate. */}
                {columns.map((p) => {
                  const proposal = row.proposals.find(
                    (pr) => pr.ordinal === p.ordinal,
                  );
                  const value = proposal?.value ?? null;
                  if (value == null) {
                    return (
                      <span
                        key={p.ordinal}
                        className="text-muted-foreground px-1.5 py-1 text-xs"
                      >
                        —
                      </span>
                    );
                  }
                  return (
                    <label
                      key={p.ordinal}
                      className={`flex cursor-pointer items-center gap-1 ${cellClass(
                        selected === String(p.ordinal),
                      )}`}
                      title={value}
                    >
                      <input
                        type="radio"
                        name={`fs-${row.field}`}
                        className="flex-none"
                        checked={selected === String(p.ordinal)}
                        onChange={() =>
                          onChangeFieldSource(row.field, p.ordinal)
                        }
                      />
                      <span className="truncate text-xs">{value}</span>
                    </label>
                  );
                })}
              </div>
            );
          })}
        </div>
      )}

      {(data.external_ids_new.length > 0 ||
        data.external_id_conflicts.length > 0) && (
        <div className="text-muted-foreground text-xs">
          External IDs:{" "}
          {data.external_ids_new.length > 0 &&
            `${data.external_ids_new.map((n) => providerLabel(n.source)).join(", ")} will be added`}
          {data.external_id_conflicts.length > 0 &&
            ` · ${data.external_id_conflicts.length} conflict(s) keep your value`}
          .
        </div>
      )}

      <div className="flex items-center justify-end gap-2">
        <Button
          onClick={onApply}
          disabled={isApplying || selectedCount === 0 || columns.length === 0}
        >
          {isApplying && <Loader2 className="mr-1.5 h-4 w-4 animate-spin" />}
          Apply merged ({selectedCount} field{selectedCount === 1 ? "" : "s"})
        </Button>
      </div>
    </div>
  );
}
