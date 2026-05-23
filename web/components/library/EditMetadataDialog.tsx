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

type FieldId =
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
  const [field, setField] = React.useState<FieldId>("language_code");
  const [value, setValue] = React.useState("");
  const [mode, setMode] = React.useState<"skip_if_set" | "replace">(
    "skip_if_set",
  );

  const def = FIELD_BY_ID.get(field)!;
  const count = issueIds.length;

  const onSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    if (count === 0) {
      onClose();
      return;
    }
    // Empty string in text fields = clear. Send `null` so the server
    // distinguishes "leave alone" from "clear".
    const patchValue = value.trim() === "" ? null : value.trim();
    const patch: BulkMetadataPatch = {};
    patch[field] = patchValue;

    update.mutate(
      { issue_ids: issueIds, patch, mode },
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
          Sets a single field across the selection. Credit fields (writer,
          penciller, …) stay per-issue; edit those one at a time.
        </DialogDescription>
      </DialogHeader>

      <div className="grid gap-4 py-4">
        <div className="grid gap-1.5">
          <Label htmlFor="emd-field">Field</Label>
          <Select
            value={field}
            onValueChange={(next) => {
              setField(next as FieldId);
              setValue("");
            }}
          >
            <SelectTrigger id="emd-field">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {FIELDS.map((f) => (
                <SelectItem key={f.id} value={f.id}>
                  {f.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>

        <div className="grid gap-1.5">
          <Label htmlFor="emd-value">New value</Label>
          {def.kind === "enum" ? (
            // Radix Select can't model an empty-string value (it's
            // reserved for "no selection"), so the clear sentinel is
            // mapped to `__clear__` in the trigger and back to `""`
            // in state.
            <Select
              value={value === "" ? "__clear__" : value}
              onValueChange={(next) =>
                setValue(next === "__clear__" ? "" : next)
              }
            >
              <SelectTrigger id="emd-value">
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
              id="emd-value"
              value={value}
              onChange={(e) => setValue(e.target.value)}
              placeholder="Leave blank to clear"
            />
          )}
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
