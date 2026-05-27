"use client";

/**
 * `<MetadataPreviewPane>` — per-field diff preview for the M5
 * metadata-apply flow. Renders one row per scalar field that the
 * candidate would touch, with a checkbox controlling whether the
 * field actually writes. Provenance tooltips show who set the
 * current value + when. External-IDs conflicts get their own
 * amber-rendered section with a Keep mine / Use theirs toggle.
 *
 * The pane is presentational — it reads `data` (via the parent's
 * `useMetadataProposedDiff*` hook) and emits `onChangeSelected` /
 * `onChangeOverrideSources` events. The parent owns the apply call.
 *
 * Plan: metadata-providers-1.0 M5 diff view (revisited 2026-05-26
 * after user feedback that scope skipping was happening silently).
 */

import { ChevronLeft, Loader2 } from "lucide-react";
import * as React from "react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import type { DiffResp, ScalarDiffRow } from "@/lib/api/types";
import { cn } from "@/lib/utils";

export type MetadataPreviewPaneProps = {
  data: DiffResp | undefined;
  isLoading: boolean;
  errorMessage: string | null;
  selectedFields: Set<string>;
  overrideExternalIdSources: Set<string>;
  onChangeSelected: (next: Set<string>) => void;
  onChangeOverrideSources: (next: Set<string>) => void;
  onBack: () => void;
  onApply: () => void;
  isApplying: boolean;
  /** Optional admin-mode signal — when false, the BlockedByUser
   *  rows render their checkbox disabled rather than implying the
   *  user could opt in. */
  canOverride: boolean;
  /** When set, BlockedByUser rows get a "Revert pin" button that
   *  clears the user pin via this callback. Returning a Promise lets
   *  the parent surface in-flight state on the button (greying out
   *  until the mutation completes); the caller is expected to refetch
   *  the diff query after success so the row re-classifies. The
   *  series-scope dialog leaves this `undefined` (series-scope pin
   *  revert isn't surfaced in M5.3). */
  onRevertPin?: (field: string) => Promise<void>;
};

/** Decision strings that map to actionable writes. Anything else
 *  renders the row with a disabled checkbox (or omits it from the
 *  default-checked set). Mirrors the server-side
 *  `DiffDecision::would_change` predicate. */
const ACTIONABLE: ReadonlySet<string> = new Set([
  "would_fill",
  "would_replace",
]);

/** Map provenance `set_by` values to a human-readable source name.
 *  The trailing "(you)" suffix lets users distinguish their own
 *  edits from a provider's at a glance. */
function provenanceLabel(setBy: string): string {
  switch (setBy) {
    case "user":
      return "user edit";
    case "comicinfo":
      return "ComicInfo.xml";
    case "metroninfo":
      return "MetronInfo.xml";
    case "comicvine":
      return "ComicVine";
    case "metron":
      return "Metron";
    case "gcd":
      return "Grand Comics Database";
    case "marvel":
      return "Marvel";
    case "locg":
      return "League of Comic Geeks";
    case "scanner_folder_tag":
      return "scanner folder tag";
    case "scanner_inference":
      return "scanner inference";
    case "cross_reference":
      return "cross-reference";
    default:
      return setBy;
  }
}

function decisionBadge(decision: string) {
  switch (decision) {
    case "would_fill":
      return (
        <Badge
          variant="default"
          className="bg-emerald-500/15 text-emerald-700 hover:bg-emerald-500/15 dark:text-emerald-400"
        >
          Will fill
        </Badge>
      );
    case "would_replace":
      return (
        <Badge
          variant="default"
          className="bg-amber-500/15 text-amber-700 hover:bg-amber-500/15 dark:text-amber-400"
        >
          Will replace
        </Badge>
      );
    case "blocked_by_user":
      return (
        <Badge
          variant="outline"
          className="border-red-500/40 text-red-600 dark:text-red-400"
        >
          User-set
        </Badge>
      );
    case "no_change":
      return (
        <Badge variant="outline" className="text-muted-foreground">
          Same
        </Badge>
      );
    case "skipped_fill_missing_has_value":
      return (
        <Badge variant="outline" className="text-muted-foreground">
          Has value
        </Badge>
      );
    case "no_incoming_value":
      return (
        <Badge variant="outline" className="text-muted-foreground">
          —
        </Badge>
      );
    default:
      return <Badge variant="outline">{decision}</Badge>;
  }
}

/** True when the user could conceivably opt this row into the apply
 *  set. False for rows where the candidate has no value or the field
 *  is admin-overridable but the caller isn't an admin. */
function rowIsToggleable(row: ScalarDiffRow, canOverride: boolean): boolean {
  if (row.decision === "no_incoming_value") return false;
  if (row.decision === "no_change") return false;
  if (row.decision === "blocked_by_user" && !canOverride) return false;
  return true;
}

export function MetadataPreviewPane({
  data,
  isLoading,
  errorMessage,
  selectedFields,
  overrideExternalIdSources,
  onChangeSelected,
  onChangeOverrideSources,
  onBack,
  onApply,
  isApplying,
  canOverride,
  onRevertPin,
}: MetadataPreviewPaneProps) {
  // Per-row reverting state so the Revert-pin button can disable
  // itself while the mutation is in flight. Tracks the field key
  // currently being cleared (or `null` when idle); the diff refetch
  // that follows clears the row out of the `blocked_by_user` bucket
  // and the row vanishes naturally.
  const [revertingField, setRevertingField] = React.useState<string | null>(
    null,
  );
  const handleRevert = async (field: string) => {
    if (!onRevertPin || revertingField) return;
    setRevertingField(field);
    try {
      await onRevertPin(field);
    } finally {
      setRevertingField(null);
    }
  };
  const toggleField = (key: string) => {
    const next = new Set(selectedFields);
    if (next.has(key)) next.delete(key);
    else next.add(key);
    onChangeSelected(next);
  };
  const toggleSource = (source: string) => {
    const next = new Set(overrideExternalIdSources);
    if (next.has(source)) next.delete(source);
    else next.add(source);
    onChangeOverrideSources(next);
  };

  if (isLoading) {
    return (
      <div className="text-muted-foreground flex items-center justify-center gap-2 py-12 text-sm">
        <Loader2 className="h-4 w-4 animate-spin" /> Computing preview…
      </div>
    );
  }
  if (errorMessage) {
    return (
      <div className="space-y-3 py-4">
        <p className="text-destructive text-sm">{errorMessage}</p>
        <Button variant="outline" size="sm" onClick={onBack}>
          <ChevronLeft className="mr-1.5 h-4 w-4" /> Back to candidates
        </Button>
      </div>
    );
  }
  if (!data) return null;

  const changesCount = data.changes_count;
  const newIdsCount = data.external_ids_new.length;
  const conflictsCount = data.external_id_conflicts.length;
  // M5.3 — suppressed-pins summary. Counts rows where the user has
  // a pin that the apply will preserve (i.e. the proposed value would
  // be blocked by the user-precedence rule). Renders alongside the
  // "N changes pending" line for symmetry.
  const suppressedPinsCount = data.rows.filter(
    (r) => r.decision === "blocked_by_user",
  ).length;

  return (
    <TooltipProvider delayDuration={200}>
      <div className="space-y-3">
        <div className="flex items-center justify-between gap-3 text-xs">
          <Button variant="ghost" size="sm" onClick={onBack} disabled={isApplying}>
            <ChevronLeft className="mr-1.5 h-4 w-4" /> Back
          </Button>
          <div className="text-muted-foreground flex flex-col items-end gap-0.5">
            <p>
              {changesCount === 0
                ? "Nothing would change with the current settings."
                : `${changesCount} change${changesCount === 1 ? "" : "s"} pending.`}
            </p>
            {suppressedPinsCount > 0 && (
              <p className="text-amber-700 dark:text-amber-400">
                {suppressedPinsCount} of your edit
                {suppressedPinsCount === 1 ? "" : "s"} will be preserved.
              </p>
            )}
          </div>
        </div>

        <div className="max-h-[55vh] overflow-y-auto pr-3">
          <div className="space-y-4 pr-1">
            {/* Scalar field rows */}
            <ul className="divide-border/40 divide-y">
              {data.rows.map((row) => {
                const toggleable = rowIsToggleable(row, canOverride);
                const checked = selectedFields.has(row.field);
                const provenance = row.current_set_by
                  ? `${provenanceLabel(row.current_set_by)}${
                      row.current_set_at
                        ? ` · ${formatProvenanceDate(row.current_set_at)}`
                        : ""
                    }`
                  : "no prior provenance";
                return (
                  <li
                    key={row.field}
                    className={cn(
                      "flex items-start gap-3 py-3",
                      row.decision === "blocked_by_user" && "bg-red-500/5",
                    )}
                  >
                    <Checkbox
                      id={`mpp-${row.field}`}
                      checked={checked}
                      disabled={!toggleable || isApplying}
                      onCheckedChange={() => toggleField(row.field)}
                      className="mt-0.5"
                    />
                    <div className="min-w-0 flex-1 space-y-1">
                      <div className="flex flex-wrap items-center gap-2">
                        <label
                          htmlFor={`mpp-${row.field}`}
                          className={cn(
                            "cursor-pointer text-sm font-medium",
                            !toggleable && "cursor-not-allowed opacity-60",
                          )}
                        >
                          {row.label}
                        </label>
                        {decisionBadge(row.decision)}
                        <Tooltip>
                          <TooltipTrigger asChild>
                            <span className="text-muted-foreground/70 cursor-help text-[10px] uppercase tracking-wider">
                              provenance
                            </span>
                          </TooltipTrigger>
                          <TooltipContent side="top">{provenance}</TooltipContent>
                        </Tooltip>
                        {row.decision === "blocked_by_user" && onRevertPin && (
                          <Button
                            type="button"
                            variant="ghost"
                            size="sm"
                            className="h-6 text-xs"
                            disabled={
                              isApplying || revertingField === row.field
                            }
                            onClick={() => void handleRevert(row.field)}
                          >
                            {revertingField === row.field ? (
                              <>
                                <Loader2 className="mr-1 h-3 w-3 animate-spin" />
                                Reverting
                              </>
                            ) : (
                              "Revert pin"
                            )}
                          </Button>
                        )}
                      </div>
                      <div className="text-muted-foreground grid grid-cols-1 gap-x-3 gap-y-0.5 text-xs sm:grid-cols-[auto_1fr]">
                        <span className="text-muted-foreground/80 uppercase tracking-wider text-[10px]">
                          Current
                        </span>
                        <span className="truncate">
                          {row.current_value ?? <em className="opacity-70">empty</em>}
                        </span>
                        <span className="text-muted-foreground/80 uppercase tracking-wider text-[10px]">
                          Proposed
                        </span>
                        <span className="truncate">
                          {row.proposed_value ?? (
                            <em className="opacity-70">empty</em>
                          )}
                        </span>
                      </div>
                    </div>
                  </li>
                );
              })}
            </ul>

            {/* External-IDs new (no conflict, just additions) */}
            {newIdsCount > 0 && (
              <section className="border-border/40 rounded border p-3 space-y-2">
                <h4 className="text-xs font-semibold uppercase tracking-wider">
                  New external IDs ({newIdsCount})
                </h4>
                <ul className="text-muted-foreground space-y-1 text-xs">
                  {data.external_ids_new.map((r) => (
                    <li key={r.source}>
                      <span className="font-mono uppercase">{r.source}</span> ·{" "}
                      {r.external_id}
                    </li>
                  ))}
                </ul>
                <p className="text-muted-foreground/80 text-[11px]">
                  These IDs will always be written on Apply — no conflict.
                </p>
              </section>
            )}

            {/* External-IDs conflicts (require explicit Use theirs) */}
            {conflictsCount > 0 && (
              <section className="rounded border border-amber-500/40 bg-amber-500/5 p-3 space-y-2">
                <h4 className="text-xs font-semibold uppercase tracking-wider text-amber-700 dark:text-amber-400">
                  External-ID conflicts ({conflictsCount})
                </h4>
                <p className="text-muted-foreground text-[11px]">
                  Your value disagrees with the candidate&rsquo;s. Pick which
                  to keep per source.
                </p>
                <ul className="space-y-2">
                  {data.external_id_conflicts.map((c) => {
                    const useTheirs = overrideExternalIdSources.has(c.source);
                    return (
                      <li
                        key={c.source}
                        className="flex flex-wrap items-center justify-between gap-2 text-xs"
                      >
                        <div>
                          <span className="font-mono uppercase">{c.source}</span>
                          <span className="text-muted-foreground">
                            {" "}
                            · yours: {c.current_external_id} · theirs:{" "}
                            {c.proposed_external_id}
                          </span>
                        </div>
                        <div className="flex items-center gap-1">
                          <Button
                            type="button"
                            size="sm"
                            variant={useTheirs ? "outline" : "default"}
                            onClick={() => toggleSource(c.source)}
                            className="h-7 text-xs"
                            disabled={isApplying}
                          >
                            Keep mine
                          </Button>
                          <Button
                            type="button"
                            size="sm"
                            variant={useTheirs ? "default" : "outline"}
                            onClick={() => toggleSource(c.source)}
                            className="h-7 text-xs"
                            disabled={isApplying}
                          >
                            Use theirs
                          </Button>
                        </div>
                      </li>
                    );
                  })}
                </ul>
              </section>
            )}
          </div>
        </div>

        <div className="flex items-center justify-end gap-2">
          <Button
            variant="outline"
            onClick={onBack}
            disabled={isApplying}
            size="sm"
          >
            Cancel
          </Button>
          <Button
            onClick={onApply}
            disabled={isApplying || (selectedFields.size === 0 && newIdsCount === 0)}
            size="sm"
          >
            {isApplying ? (
              <>
                <Loader2 className="mr-1.5 h-4 w-4 animate-spin" /> Applying
              </>
            ) : (
              `Apply ${selectedFields.size} ${selectedFields.size === 1 ? "change" : "changes"}`
            )}
          </Button>
        </div>
      </div>
    </TooltipProvider>
  );
}

/** Default-checked state — every actionable scalar starts checked +
 *  the new external_ids are always written. Conflicts default to
 *  "Keep mine" (empty override set). Exported so the dialog parent
 *  can call it once when transitioning into the preview state. */
export function defaultSelectedFields(diff: DiffResp): Set<string> {
  const out = new Set<string>();
  for (const row of diff.rows) {
    if (ACTIONABLE.has(row.decision)) out.add(row.field);
  }
  return out;
}

function formatProvenanceDate(iso: string): string {
  try {
    const d = new Date(iso);
    if (Number.isNaN(d.getTime())) return iso;
    // Compact, like "2026-04-15". Full ISO is too noisy for a tooltip.
    return d.toISOString().slice(0, 10);
  } catch {
    return iso;
  }
}
