"use client";

import { Check, Loader2, Pencil, X } from "lucide-react";
import { useRouter } from "next/navigation";
import * as React from "react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
import { useUpdateIssue } from "@/lib/api/mutations";
import { useMe } from "@/lib/api/queries";
import type { UpdateIssueReq } from "@/lib/api/types";

/** The scalar issue fields a single inline popover can PATCH. Junction /
 *  credit / cover fields are deliberately excluded — they need the full
 *  edit sheet's multi-value editors (audit B12 decision). */
export type InlineIssueField = Extract<
  keyof UpdateIssueReq,
  | "publisher"
  | "imprint"
  | "alternate_series"
  | "story_arc"
  | "story_arc_number"
  | "volume"
  | "summary"
  | "notes"
  | "language_code"
  | "age_rating"
  | "format"
>;

type Kind = "text" | "textarea" | "number" | "enum";

type FieldConfig = {
  kind: Kind;
  label: string;
  options?: { value: string; label: string }[];
};

/** Per-field editor config (kind + enum options) so a caller only needs to
 *  pass `field` + the current value. Enum option sets mirror the bulk
 *  `EditMetadataDialog` so inline + bulk edits offer the same choices. */
const FIELD_CONFIG: Record<InlineIssueField, FieldConfig> = {
  publisher: { kind: "text", label: "Publisher" },
  imprint: { kind: "text", label: "Imprint" },
  alternate_series: { kind: "text", label: "Alternate series" },
  story_arc: { kind: "text", label: "Story arc" },
  story_arc_number: { kind: "text", label: "Story arc number" },
  volume: { kind: "number", label: "Volume" },
  summary: { kind: "textarea", label: "Summary" },
  notes: { kind: "textarea", label: "Notes" },
  language_code: {
    kind: "enum",
    label: "Language",
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
  age_rating: {
    kind: "enum",
    label: "Age rating",
    options: [
      { value: "Everyone", label: "Everyone" },
      { value: "Teen", label: "Teen" },
      { value: "Mature 17+", label: "Mature 17+" },
      { value: "Adults Only 18+", label: "Adults Only 18+" },
    ],
  },
  format: {
    kind: "enum",
    label: "Format",
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
};

/** Build the single-field PATCH body. An empty string clears the field
 *  (`null`); number fields parse to an int or clear; everything else sends
 *  the trimmed string. Pure + exported for unit tests. */
export function buildFieldPatch(
  field: InlineIssueField,
  raw: string,
  kind: Kind,
): Partial<UpdateIssueReq> {
  const trimmed = raw.trim();
  if (trimmed === "") return { [field]: null } as Partial<UpdateIssueReq>;
  if (kind === "number") {
    const n = Number.parseInt(trimmed, 10);
    return {
      [field]: Number.isFinite(n) ? n : null,
    } as Partial<UpdateIssueReq>;
  }
  return { [field]: trimmed } as Partial<UpdateIssueReq>;
}

/**
 * Inline single-field metadata editor (audit B12). Renders the field's
 * current value; for admins it adds a pencil that opens a popover to PATCH
 * just that one field — no full 30-field sheet for a one-word fix. The
 * issue PATCH is `RequireAdmin`, so non-admins see the value only.
 *
 * Used as the `value` of a `MetadataGrid` row on the issue Details tab; the
 * caller only mounts it for fields that already have a value, so the grid
 * stays compact (adding a missing field still goes through the sheet).
 */
export function InlineIssueFieldEdit({
  seriesSlug,
  issueSlug,
  field,
  value,
  display,
}: {
  seriesSlug: string;
  issueSlug: string;
  field: InlineIssueField;
  /** Current raw value (string form) used to seed the editor. */
  value: string;
  /** Optional pre-formatted display node (e.g. a badge); falls back to the
   *  raw `value` text. */
  display?: React.ReactNode;
}) {
  const { kind, label, options } = FIELD_CONFIG[field];
  const me = useMe();
  const isAdmin = me.data?.role === "admin";
  const router = useRouter();
  const update = useUpdateIssue(seriesSlug, issueSlug);
  const [open, setOpen] = React.useState(false);
  const [draft, setDraft] = React.useState(value);

  const shown = display ?? value;
  if (!isAdmin) return <>{shown}</>;

  // Re-seed the draft from the current value each time the popover opens, so
  // a save elsewhere (or a rescan) can't leave a stale edit queued.
  function onOpenChange(next: boolean) {
    if (next) setDraft(value);
    setOpen(next);
  }

  function save() {
    update.mutate(buildFieldPatch(field, draft, kind), {
      onSuccess: () => {
        // The issue page is an RSC — refresh to re-render the new value.
        router.refresh();
        setOpen(false);
      },
    });
  }

  return (
    <span className="group/inline inline-flex items-center gap-1.5">
      <span>{shown}</span>
      <Popover open={open} onOpenChange={onOpenChange}>
        <PopoverTrigger asChild>
          <button
            type="button"
            aria-label={`Edit ${label}`}
            className="text-muted-foreground hover:text-foreground opacity-0 transition-opacity group-hover/inline:opacity-100 focus-visible:opacity-100"
          >
            <Pencil className="size-3.5" />
          </button>
        </PopoverTrigger>
        <PopoverContent align="start" className="w-72 space-y-2">
          <p className="text-xs font-medium">{label}</p>
          {kind === "enum" && options ? (
            <Select value={draft} onValueChange={setDraft}>
              <SelectTrigger className="h-9">
                <SelectValue placeholder="—" />
              </SelectTrigger>
              <SelectContent>
                {options.map((o) => (
                  <SelectItem key={o.value} value={o.value}>
                    {o.label}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          ) : kind === "textarea" ? (
            <Textarea
              value={draft}
              onChange={(e) => setDraft(e.target.value)}
              rows={4}
              autoFocus
            />
          ) : (
            <Input
              type={kind === "number" ? "number" : "text"}
              inputMode={kind === "number" ? "numeric" : undefined}
              value={draft}
              onChange={(e) => setDraft(e.target.value)}
              autoFocus
              onKeyDown={(e) => {
                if (e.key === "Enter") {
                  e.preventDefault();
                  save();
                }
              }}
            />
          )}
          <div className="flex items-center justify-end gap-1.5">
            <Button
              type="button"
              variant="ghost"
              size="sm"
              onClick={() => setOpen(false)}
              disabled={update.isPending}
            >
              <X className="size-3.5" /> Cancel
            </Button>
            <Button
              type="button"
              size="sm"
              onClick={save}
              disabled={update.isPending || draft === value}
            >
              {update.isPending ? (
                <Loader2 className="size-3.5 animate-spin" />
              ) : (
                <Check className="size-3.5" />
              )}
              Save
            </Button>
          </div>
        </PopoverContent>
      </Popover>
    </span>
  );
}
