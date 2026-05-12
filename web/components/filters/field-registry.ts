/**
 * Mirrors `crates/server/src/views/registry.rs` — the server is the
 * source of truth, this file is the typed projection the FilterBuilder
 * uses to render field/op/value editors.
 *
 * Kept hand-written rather than generated because the per-field UI
 * editor choice (which `ValueEditor` to render, how to fetch options)
 * lives entirely on the client. The shape of `id`, `kind`, and
 * `allowed_ops` must match the server registry — a drifted entry will
 * hit a 422 on save with a clear validation error.
 */
import type { Field, Op } from "@/lib/api/types";

/** High-level value family. Drives which editor renders + what shape
 *  the `Condition.value` has on the wire. */
export type FieldKind = "text" | "number" | "date" | "enum" | "uuid" | "multi";

export type OptionsEndpoint =
  | { kind: "genres" }
  | { kind: "tags" }
  | { kind: "credits"; role: string }
  | { kind: "libraries" }
  | { kind: "publishers" }
  | { kind: "languages" }
  | { kind: "age_ratings" }
  | { kind: "characters" }
  | { kind: "teams" }
  | { kind: "locations" };

export type FieldSpec = {
  id: Field;
  label: string;
  kind: FieldKind;
  allowedOps: Op[];
  /** Closed list of legal scalar values. Empty unless `kind === 'enum'`. */
  enumValues?: readonly string[];
  /** Where the `MultiSelectEditor` (or library-Combobox) fetches its
   *  options. `undefined` when there is no remote lookup. */
  optionsEndpoint?: OptionsEndpoint;
};

const TEXT_OPS: Op[] = ["contains", "starts_with", "equals", "not_equals"];
const NUMBER_OPS: Op[] = [
  "equals",
  "not_equals",
  "gt",
  "gte",
  "lt",
  "lte",
  "between",
];
const DATE_OPS: Op[] = ["before", "after", "between", "relative", "lt", "gt"];
const ENUM_OPS: Op[] = ["is", "is_not", "in", "not_in"];
const MULTI_OPS: Op[] = ["includes_any", "includes_all", "excludes"];

const SERIES_STATUS_VALUES = [
  "continuing",
  "ended",
  "cancelled",
  "hiatus",
  "limited",
] as const;

const AGE_RATING_VALUES = [
  "Unknown",
  "Adults Only 18+",
  "Early Childhood",
  "Everyone",
  "Everyone 10+",
  "G",
  "Kids to Adults",
  "M",
  "MA15+",
  "Mature 17+",
  "PG",
  "R18+",
  "Rating Pending",
  "Teen",
  "X18+",
] as const;

export const FIELD_SPECS: readonly FieldSpec[] = [
  {
    id: "library",
    label: "Library",
    kind: "uuid",
    allowedOps: ["equals", "not_equals", "in", "not_in"],
    optionsEndpoint: { kind: "libraries" },
  },
  { id: "name", label: "Name", kind: "text", allowedOps: TEXT_OPS },
  { id: "year", label: "Year", kind: "number", allowedOps: NUMBER_OPS },
  { id: "volume", label: "Volume", kind: "number", allowedOps: NUMBER_OPS },
  {
    id: "total_issues",
    label: "Total Issues",
    kind: "number",
    allowedOps: NUMBER_OPS,
  },
  { id: "publisher", label: "Publisher", kind: "text", allowedOps: TEXT_OPS },
  { id: "imprint", label: "Imprint", kind: "text", allowedOps: TEXT_OPS },
  {
    id: "status",
    label: "Status",
    kind: "enum",
    allowedOps: ENUM_OPS,
    enumValues: SERIES_STATUS_VALUES,
  },
  {
    id: "age_rating",
    label: "Age Rating",
    kind: "enum",
    allowedOps: ENUM_OPS,
    enumValues: AGE_RATING_VALUES,
  },
  {
    id: "language_code",
    label: "Language",
    kind: "text",
    allowedOps: TEXT_OPS,
  },
  { id: "created_at", label: "Created At", kind: "date", allowedOps: DATE_OPS },
  { id: "updated_at", label: "Updated At", kind: "date", allowedOps: DATE_OPS },
  {
    id: "genres",
    label: "Genres",
    kind: "multi",
    allowedOps: MULTI_OPS,
    optionsEndpoint: { kind: "genres" },
  },
  {
    id: "tags",
    label: "Tags",
    kind: "multi",
    allowedOps: MULTI_OPS,
    optionsEndpoint: { kind: "tags" },
  },
  {
    id: "writer",
    label: "Writers",
    kind: "multi",
    allowedOps: MULTI_OPS,
    optionsEndpoint: { kind: "credits", role: "writer" },
  },
  {
    id: "penciller",
    label: "Pencillers",
    kind: "multi",
    allowedOps: MULTI_OPS,
    optionsEndpoint: { kind: "credits", role: "penciller" },
  },
  {
    id: "inker",
    label: "Inkers",
    kind: "multi",
    allowedOps: MULTI_OPS,
    optionsEndpoint: { kind: "credits", role: "inker" },
  },
  {
    id: "colorist",
    label: "Colorists",
    kind: "multi",
    allowedOps: MULTI_OPS,
    optionsEndpoint: { kind: "credits", role: "colorist" },
  },
  {
    id: "letterer",
    label: "Letterers",
    kind: "multi",
    allowedOps: MULTI_OPS,
    optionsEndpoint: { kind: "credits", role: "letterer" },
  },
  {
    id: "cover_artist",
    label: "Cover Artists",
    kind: "multi",
    allowedOps: MULTI_OPS,
    optionsEndpoint: { kind: "credits", role: "cover_artist" },
  },
  {
    id: "editor",
    label: "Editors",
    kind: "multi",
    allowedOps: MULTI_OPS,
    optionsEndpoint: { kind: "credits", role: "editor" },
  },
  {
    id: "translator",
    label: "Translators",
    kind: "multi",
    allowedOps: MULTI_OPS,
    optionsEndpoint: { kind: "credits", role: "translator" },
  },
  {
    id: "read_progress",
    label: "Read Progress",
    kind: "number",
    allowedOps: NUMBER_OPS,
  },
  { id: "last_read", label: "Last Read", kind: "date", allowedOps: DATE_OPS },
  {
    id: "read_count",
    label: "Read Count",
    kind: "number",
    allowedOps: NUMBER_OPS,
  },
] as const;

export function specFor(field: Field): FieldSpec {
  const spec = FIELD_SPECS.find((s) => s.id === field);
  if (!spec) throw new Error(`Unknown field: ${field}`);
  return spec;
}

export const OP_LABELS: Record<Op, string> = {
  contains: "contains",
  starts_with: "starts with",
  equals: "equals",
  not_equals: "does not equal",
  is: "is",
  is_not: "is not",
  in: "is one of",
  not_in: "is not one of",
  gt: ">",
  gte: "≥",
  lt: "<",
  lte: "≤",
  between: "between",
  before: "before",
  after: "after",
  relative: "in last",
  includes_any: "any of",
  includes_all: "all of",
  excludes: "none of",
  is_true: "is true",
  is_false: "is false",
};
