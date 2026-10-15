"use client";

import * as React from "react";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  useBulkUpdateMetadata,
  type BulkMetadataPatch,
} from "@/lib/api/mutations";

/**
 * "Edit metadata…" — `manga-and-bulk-metadata-1.0` M5.
 *
 * Wired from the SelectionToolbar overflow on every list surface that
 * already has multi-select enabled. Operates on the current
 * selection via `PATCH /me/issues/bulk-metadata`.
 *
 * Field set is intentionally narrow:
 *   language_code, manga, publisher, imprint, age_rating, format,
 *   genre, tags, story_arc
 *
 * Credit fields (writer, penciller, …) are deliberately omitted —
 * they vary issue-to-issue (guest artists, variant covers, mid-run
 * translator changes) and bulk-editing them risks clobbering
 * accurate per-issue data.
 */

export type FieldId =
  | "language_code"
  | "manga"
  | "publisher"
  | "imprint"
  | "age_rating"
  | "format"
  | "genre"
  | "tags"
  | "story_arc";

/// Discriminated union on `kind` so the `enum` branch's `options`
/// field is non-optional structurally. Killed the
/// `def.options!.map(...)` non-null assertion in render. M5 of
/// code-quality-cleanup-1.0.
type FieldDef =
  | {
      id: FieldId;
      label: string;
      kind: "enum";
      /** Closed value set the dropdown surfaces. */
      options: Array<{ value: string; label: string }>;
    }
  | {
      id: FieldId;
      label: string;
      kind: "text";
    };

const FIELDS: FieldDef[] = [
  {
    id: "language_code",
    label: "Language",
    kind: "enum",
    options: [
      { value: "en", label: "English (en)" },
      { value: "ja", label: "Japanese (ja)" },
      { value: "fr", label: "French (fr)" },
      { value: "es", label: "Spanish (es)" },
      { value: "de", label: "German (de)" },
      { value: "it", label: "Italian (it)" },
      { value: "pt", label: "Portuguese (pt)" },
      { value: "ko", label: "Korean (ko)" },
      { value: "zh", label: "Chinese (zh)" },
    ],
  },
  {
    id: "manga",
    label: "Manga (reading direction)",
    kind: "enum",
    options: [
      { value: "No", label: "No (left-to-right)" },
      { value: "Yes", label: "Yes" },
      {
        value: "YesAndRightToLeft",
        label: "Yes — right-to-left (manga)",
      },
    ],
  },
  { id: "publisher", label: "Publisher", kind: "text" },
  { id: "imprint", label: "Imprint", kind: "text" },
  {
    id: "age_rating",
    label: "Age rating",
    kind: "enum",
    options: [
      { value: "Everyone", label: "Everyone" },
      { value: "Teen", label: "Teen" },
      { value: "Mature 17+", label: "Mature 17+" },
      { value: "Adults Only 18+", label: "Adults Only 18+" },
    ],
  },
  {
    id: "format",
    label: "Format",
    kind: "enum",
    options: [
      { value: "Series", label: "Series" },
      { value: "Trade Paperback", label: "Trade Paperback" },
      { value: "Annual", label: "Annual" },
      { value: "Special", label: "Special" },
      { value: "Limited Series", label: "Limited Series" },
      { value: "Mini-Series", label: "Mini-Series" },
      { value: "One-Shot", label: "One-Shot" },
      { value: "Graphic Novel", label: "Graphic Novel" },
    ],
  },
  { id: "genre", label: "Genre (CSV)", kind: "text" },
  { id: "tags", label: "Tags (CSV)", kind: "text" },
  { id: "story_arc", label: "Story arc", kind: "text" },
];

const FIELD_BY_ID = new Map(FIELDS.map((f) => [f.id, f]));

export function EditMetadataDialog({
  open,
  onOpenChange,
  issueIds,
}: {
  open: boolean;
  onOpenChange: (next: boolean) => void;
  issueIds: string[];
}) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md">
        <EditMetadataForm
          issueIds={issueIds}
          onClose={() => onOpenChange(false)}
        />
      </DialogContent>
    </Dialog>
  );
}

/** One field + new value in the multi-field editor (post-audit T3.7). */
export type FieldRow = { field: FieldId; value: string };

/** Fold the editor rows into one `BulkMetadataPatch`. Empty string in a
 *  field = clear: sent as `null` so the server distinguishes "leave
 *  alone" (key absent) from "clear". Later rows win on a duplicate field
 *  (the UI prevents duplicates; this keeps the function total). Exported
 *  for tests. */
export function buildBulkPatch(
  rows: ReadonlyArray<FieldRow>,
): BulkMetadataPatch {
  const patch: BulkMetadataPatch = {};
  for (const row of rows) {
    const trimmed = row.value.trim();
    patch[row.field] = trimmed === "" ? null : trimmed;
  }
  return patch;
}

/**
 * The dialog's inner form. Exported separately so vitest can render
 * it without going through Radix's portal layer
 * (`renderToStaticMarkup` doesn't traverse portals, so testing the
 * shell directly produces empty output).
 */
export function EditMetadataForm({
  issueIds,
  onClose,
}: {
  issueIds: string[];
  /** Called after a successful apply, and from the Cancel button. */
  onClose: () => void;
}) {
  const update = useBulkUpdateMetadata();
  // Post-audit T3.7: several fields per apply. Each row is one field +
  // its new value (empty = clear); the rows build one PATCH, so the
  // server applies them in one pass with one audit row and one summary.
  const [rows, setRows] = React.useState<FieldRow[]>([
    { field: "language_code", value: "" },
  ]);
  const [mode, setMode] = React.useState<"skip_if_set" | "replace">(
    "skip_if_set",
  );
  const count = issueIds.length;
  const usedFields = new Set(rows.map((r) => r.field));
  const canAddRow = rows.length < FIELDS.length;

  const setRow = (i: number, next: FieldRow) =>
    setRows((prev) => prev.map((r, j) => (j === i ? next : r)));
  const removeRow = (i: number) =>
    setRows((prev) =>
      prev.length > 1 ? prev.filter((_, j) => j !== i) : prev,
    );
  const addRow = () => {
    const free = FIELDS.find((f) => !usedFields.has(f.id));
    if (free) setRows((prev) => [...prev, { field: free.id, value: "" }]);
  };

  const onSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    if (count === 0) {
      onClose();
      return;
    }
    update.mutate(
      { issue_ids: issueIds, patch: buildBulkPatch(rows), mode },
      {
        onSuccess: () => onClose(),
      },
    );
  };

  return (
    <form onSubmit={onSubmit}>
      <DialogHeader>
        <DialogTitle>
          Edit {count} issue{count === 1 ? "" : "s"}
        </DialogTitle>
        <DialogDescription>
          Sets one or more fields across the selection in a single apply. Credit
          fields (writer, penciller, …) stay per-issue; edit those one at a
          time.
        </DialogDescription>
      </DialogHeader>

      <div className="grid gap-4 py-4">
        {rows.map((row, i) => {
          const def = FIELD_BY_ID.get(row.field)!;
          const fieldId = `emd-field-${i}`;
          const valueId = `emd-value-${i}`;
          return (
            <div
              key={i}
              className="grid gap-3 sm:grid-cols-[1fr_1fr_auto] sm:items-end"
              data-testid="emd-row"
            >
              <div className="grid gap-1.5">
                <Label htmlFor={fieldId}>Field</Label>
                <Select
                  value={row.field}
                  onValueChange={(next) =>
                    setRow(i, { field: next as FieldId, value: "" })
                  }
                >
                  <SelectTrigger id={fieldId}>
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {FIELDS.map((f) => (
                      <SelectItem
                        key={f.id}
                        value={f.id}
                        // A field appears in one row at most.
                        disabled={f.id !== row.field && usedFields.has(f.id)}
                      >
                        {f.label}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>

              <div className="grid gap-1.5">
                <Label htmlFor={valueId}>New value</Label>
                {def.kind === "enum" ? (
                  // Radix Select can't model an empty-string value (it's
                  // reserved for "no selection"), so the clear sentinel is
                  // mapped to `__clear__` in the trigger and back to `""`
                  // in state.
                  <Select
                    value={row.value === "" ? "__clear__" : row.value}
                    onValueChange={(next) =>
                      setRow(i, {
                        ...row,
                        value: next === "__clear__" ? "" : next,
                      })
                    }
                  >
                    <SelectTrigger id={valueId}>
                      <SelectValue placeholder="— Clear —" />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value="__clear__">— Clear —</SelectItem>
                      {def.options.map((o) => (
                        <SelectItem key={o.value} value={o.value}>
                          {o.label}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                ) : (
                  <Input
                    id={valueId}
                    value={row.value}
                    onChange={(e) =>
                      setRow(i, { ...row, value: e.target.value })
                    }
                    placeholder="Leave blank to clear"
                  />
                )}
              </div>

              <Button
                type="button"
                variant="ghost"
                size="sm"
                onClick={() => removeRow(i)}
                disabled={rows.length === 1}
                aria-label={`Remove ${def.label}`}
              >
                Remove
              </Button>
            </div>
          );
        })}
        <div>
          <Button
            type="button"
            variant="outline"
            size="sm"
            onClick={addRow}
            disabled={!canAddRow}
          >
            Add another field
          </Button>
        </div>

        <fieldset className="grid gap-2">
          <legend className="text-foreground text-sm font-medium">Mode</legend>
          <RadioGroup
            name="emd-mode"
            value={mode}
            onValueChange={(v) => setMode(v as "skip_if_set" | "replace")}
            className="gap-2"
          >
            <Label
              htmlFor="emd-mode-skip"
              className="flex items-start gap-2 text-sm font-normal"
            >
              <RadioGroupItem
                id="emd-mode-skip"
                value="skip_if_set"
                className="mt-0.5"
              />
              <span>
                <span className="font-medium">Skip already-set</span>
                <span className="text-muted-foreground block text-xs">
                  Only update issues where the field is currently empty.
                  Recommended.
                </span>
              </span>
            </Label>
            <Label
              htmlFor="emd-mode-replace"
              className="flex items-start gap-2 text-sm font-normal"
            >
              <RadioGroupItem
                id="emd-mode-replace"
                value="replace"
                className="mt-0.5"
              />
              <span>
                <span className="font-medium">Replace existing values</span>
                <span className="text-muted-foreground block text-xs">
                  Overwrite every selected issue regardless of current value.
                </span>
              </span>
            </Label>
          </RadioGroup>
        </fieldset>
      </div>

      <DialogFooter>
        <Button
          type="button"
          variant="ghost"
          onClick={onClose}
          disabled={update.isPending}
        >
          Cancel
        </Button>
        <Button type="submit" disabled={update.isPending || count === 0}>
          {update.isPending
            ? "Applying…"
            : `Apply to ${count} issue${count === 1 ? "" : "s"}`}
        </Button>
      </DialogFooter>
    </form>
  );
}
