"use client";

/**
 * `<RelationshipFormDialog>` — WP-7.7 add / edit form for a series
 * relationship (admin only), in a dialog so the Related tab never
 * reflows around an inline form.
 *
 * - **Add** (`mode="create"`): the grouped kind picker, a target toggle
 *   "Series | Story arc | Not in library" (arc only for arc-capable kinds —
 *   `tie_in_to`) with a typeahead for each (`/series?q=` and `/arcs?q=`,
 *   both cursor-paginated with "More results"), then the scope the kind
 *   takes. `POST /series/{slug}/relationships`. WP-7.8: "Not in library"
 *   links a provider series the library doesn't have (provider + numeric
 *   id + name + optional year; qualifier only) —
 *   `POST /series/{slug}/external-relationships`.
 * - **Edit** (`mode="edit"`): kind (an arc edge keeps arc-capable kinds),
 *   from / to ranges, coverage (only for kinds that allow it), qualifier /
 *   role (the kind's allowed set from the catalogue) and note.
 *   `PATCH /series/{slug}/relationships/{id}`; omitted-vs-null semantics
 *   are avoided by always sending every scope field (`null` clears).
 *
 * Server 422s bind to the inputs through `applyServerErrors` (the field
 * names match the request body: `kind`, `target`, `target_arc`,
 * `from_range`, `to_range`, `coverage`, `qualifier`, `note`); the
 * mutation hook still toasts the summary.
 */

import * as RadioGroupPrimitive from "@radix-ui/react-radio-group";
import { Loader2 } from "lucide-react";
import * as React from "react";
import { useForm, useFormContext, useWatch, type Path } from "react-hook-form";

import { RelationshipKindSelect } from "@/components/library/RelationshipKindSelect";
import { Button } from "@/components/ui/button";
import {
  Command,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from "@/components/ui/command";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  Form,
  FormControl,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
} from "@/components/ui/form";
import { Input } from "@/components/ui/input";
import {
  Popover,
  PopoverContent,
  PopoverPortalContainer,
  PopoverTrigger,
} from "@/components/ui/popover";
import { ScrollArea } from "@/components/ui/scroll-area";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { applyServerErrors } from "@/lib/api/form-errors";
import {
  useCreateExternalRelationship,
  useCreateSeriesRelationship,
  useUpdateSeriesRelationship,
} from "@/lib/api/mutations";
import {
  useEntityListInfinite,
  useRelationshipKinds,
  useSeriesListInfinite,
} from "@/lib/api/queries";
import type {
  ExternalSource,
  RelationshipCatalogue,
  RelationshipCoverage,
  RelationshipKind,
  RelationshipQualifier,
} from "@/lib/api/types";
import { kindInfo } from "@/lib/relationships";

/** A picked target: a series or a story arc (id + display name). */
export type TargetRef = { id: string; name: string; detail?: string | null };

export type RelationshipFormValues = {
  kind: RelationshipKind;
  target_type: "series" | "arc" | "external";
  target: TargetRef | null;
  target_arc: TargetRef | null;
  qualifier: RelationshipQualifier | "";
  coverage: RelationshipCoverage | "";
  from_range: string;
  to_range: string;
  note: string;
  /** WP-7.8 "Not in library" target: provider, its numeric series id,
   *  display name and optional year (field names match the request). */
  source: ExternalSource;
  provider_series_id: string;
  name: string;
  year: string;
};

/** Providers an external link may point at. */
export const EXTERNAL_SOURCES: ReadonlyArray<{
  value: ExternalSource;
  label: string;
}> = [
  { value: "metron", label: "Metron" },
  { value: "comicvine", label: "ComicVine" },
  { value: "gcd", label: "GCD" },
];

/** Server field names the form owns (anything else lands on the root). */
const FORM_FIELDS: ReadonlyArray<Path<RelationshipFormValues>> = [
  "kind",
  "target",
  "target_arc",
  "qualifier",
  "coverage",
  "from_range",
  "to_range",
  "note",
  "source",
  "provider_series_id",
  "name",
  "year",
];

/** The relationship being edited (one row of the Related tab). */
export type EditableRelationship = {
  id: string;
  kind: RelationshipKind;
  /** An arc edge (`SeriesArcRelationshipView`): kind stays arc-capable. */
  isArc: boolean;
  /** The other end's display name, for the dialog title. */
  otherName: string;
  qualifier?: RelationshipQualifier | null;
  coverage?: RelationshipCoverage | null;
  from_range?: string | null;
  to_range?: string | null;
  note?: string | null;
};

/** Drop scope the kind doesn't accept (the server would 422). */
export function scopeForKind(
  catalogue: RelationshipCatalogue | undefined,
  kind: RelationshipKind,
  values: Pick<RelationshipFormValues, "qualifier" | "coverage">,
): Pick<RelationshipFormValues, "qualifier" | "coverage"> {
  const info = kindInfo(catalogue, kind);
  return {
    qualifier: info?.qualifiers.some((q) => q.value === values.qualifier)
      ? values.qualifier
      : "",
    coverage: info?.allows_coverage ? values.coverage : "",
  };
}

/** The `PATCH` body for an edit: every field present (`null` clears). */
export function patchBody(
  values: RelationshipFormValues,
  catalogue: RelationshipCatalogue | undefined,
) {
  const scope = scopeForKind(catalogue, values.kind, values);
  return {
    kind: values.kind,
    qualifier: scope.qualifier || null,
    coverage: scope.coverage || null,
    from_range: values.from_range.trim() || null,
    to_range: values.to_range.trim() || null,
    note: values.note.trim() || null,
  };
}

function defaults(edit?: EditableRelationship): RelationshipFormValues {
  return {
    kind: edit?.kind ?? "sequel_of",
    target_type: edit?.isArc ? "arc" : "series",
    target: null,
    target_arc: null,
    qualifier: edit?.qualifier ?? "",
    coverage: edit?.coverage ?? "",
    from_range: edit?.from_range ?? "",
    to_range: edit?.to_range ?? "",
    note: edit?.note ?? "",
    source: "metron",
    provider_series_id: "",
    name: "",
    year: "",
  };
}

/** The `POST /external-relationships` body for the "Not in library"
 *  target, or the client-side field errors that block it. */
export function externalBody(
  values: RelationshipFormValues,
  catalogue: RelationshipCatalogue | undefined,
):
  | {
      ok: true;
      body: {
        kind: RelationshipKind;
        qualifier: RelationshipQualifier | null;
        source: ExternalSource;
        provider_series_id: string;
        name: string;
        year: number | null;
      };
    }
  | {
      ok: false;
      errors: Array<{ field: Path<RelationshipFormValues>; message: string }>;
    } {
  const errors: Array<{
    field: Path<RelationshipFormValues>;
    message: string;
  }> = [];
  const id = values.provider_series_id.trim();
  const name = values.name.trim();
  const yearText = values.year.trim();
  const year = yearText ? Number(yearText) : null;
  if (!/^\d{1,12}$/.test(id)) {
    errors.push({
      field: "provider_series_id",
      message: "Enter the provider's numeric series id",
    });
  }
  if (!name) errors.push({ field: "name", message: "Enter the series name" });
  if (
    year !== null &&
    !(Number.isInteger(year) && year >= 1800 && year <= 2200)
  ) {
    errors.push({ field: "year", message: "Enter a year like 2018" });
  }
  if (errors.length > 0) return { ok: false, errors };
  const scope = scopeForKind(catalogue, values.kind, values);
  return {
    ok: true,
    body: {
      kind: values.kind,
      qualifier: scope.qualifier || null,
      source: values.source,
      provider_series_id: id,
      name,
      year,
    },
  };
}

export function RelationshipFormDialog({
  open,
  onOpenChange,
  seriesSlug,
  seriesId,
  seriesName,
  edit,
  onCloseAutoFocus,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  seriesSlug: string;
  seriesId: string;
  seriesName?: string;
  /** Set to edit an existing relationship; omit to add one. */
  edit?: EditableRelationship;
  /** Radix close-focus hook — the dialog is controlled (no trigger), so
   *  the caller hands focus back to the button that opened it
   *  (`useReturnFocus`). */
  onCloseAutoFocus?: (e: Event) => void;
}) {
  const [portal, setPortal] = React.useState<HTMLElement | null>(null);
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        ref={setPortal}
        onCloseAutoFocus={onCloseAutoFocus}
        // The dialog itself never scrolls: pickers portal into it (so the
        // modal focus trap keeps their search inputs live) and must be
        // able to overflow it. Only the form body below scrolls, and only
        // when the viewport is shorter than the form.
        className="flex max-h-[calc(100dvh-2rem)] flex-col overflow-visible sm:max-w-xl"
      >
        <DialogHeader>
          <DialogTitle>
            {edit ? `Edit relationship` : "Add relationship"}
          </DialogTitle>
          <DialogDescription>
            {edit
              ? `${seriesName ?? "This series"} → ${edit.otherName}. Both directions update together.`
              : "Link a series, a story arc, or a series you don’t have yet. A series link gets its reverse link on the other series automatically."}
          </DialogDescription>
        </DialogHeader>
        <PopoverPortalContainer value={portal}>
          {/* `-m-1 p-1`: ring room on all four sides, so the focus rings
              of the first field and the footer buttons (ring-2 + offset-2)
              aren't shaved off by this scroller's edges. */}
          <div className="-m-1 min-h-0 flex-1 overflow-y-auto p-1">
            {open ? (
              <RelationshipForm
                // Fresh form state per opened row.
                key={edit?.id ?? "new"}
                seriesSlug={seriesSlug}
                seriesId={seriesId}
                edit={edit}
                onDone={() => onOpenChange(false)}
              />
            ) : null}
          </div>
        </PopoverPortalContainer>
      </DialogContent>
    </Dialog>
  );
}

export function RelationshipForm({
  seriesSlug,
  seriesId,
  edit,
  onDone,
}: {
  seriesSlug: string;
  seriesId: string;
  edit?: EditableRelationship;
  onDone: () => void;
}) {
  const catalogue = useRelationshipKinds();
  const create = useCreateSeriesRelationship(seriesSlug);
  const createExternal = useCreateExternalRelationship(seriesSlug);
  const update = useUpdateSeriesRelationship(seriesSlug);
  const pending =
    create.isPending || update.isPending || createExternal.isPending;
  const form = useForm<RelationshipFormValues>({
    defaultValues: defaults(edit),
  });
  const kind = useWatch({ control: form.control, name: "kind" });
  const targetType = useWatch({ control: form.control, name: "target_type" });
  const info = kindInfo(catalogue.data, kind);
  const qualifiers = info?.qualifiers ?? [];
  const allowsCoverage = info?.allows_coverage ?? false;
  const allowsArc = info?.allows_arc_target ?? false;
  const isTieIn = kind === "tie_in_to" || kind === "has_tie_in";
  const isExternal = !edit && targetType === "external";

  const onKind = (k: RelationshipKind) => {
    form.setValue("kind", k, { shouldDirty: true });
    const scope = scopeForKind(catalogue.data, k, form.getValues());
    form.setValue("qualifier", scope.qualifier, { shouldDirty: true });
    form.setValue("coverage", scope.coverage, { shouldDirty: true });
    if (
      !kindInfo(catalogue.data, k)?.allows_arc_target &&
      form.getValues("target_type") === "arc"
    ) {
      form.setValue("target_type", "series");
    }
    form.clearErrors();
  };

  const onSubmit = form.handleSubmit((values) => {
    const onError = (err: unknown) =>
      applyServerErrors(form.setError, err, FORM_FIELDS);
    if (edit) {
      update.mutate(
        { id: edit.id, body: patchBody(values, catalogue.data) },
        { onSuccess: onDone, onError },
      );
      return;
    }
    if (values.target_type === "external") {
      const ext = externalBody(values, catalogue.data);
      if (!ext.ok) {
        for (const e of ext.errors) {
          form.setError(e.field, { type: "validate", message: e.message });
        }
        return;
      }
      createExternal.mutate(ext.body, { onSuccess: onDone, onError });
      return;
    }
    const arc = values.target_type === "arc" && allowsArc;
    const picked = arc ? values.target_arc : values.target;
    if (!picked) {
      form.setError(arc ? "target_arc" : "target", {
        type: "required",
        message: arc ? "Choose a story arc" : "Choose a series",
      });
      return;
    }
    const scope = scopeForKind(catalogue.data, values.kind, values);
    create.mutate(
      {
        ...(arc ? { target_arc: picked.id } : { target: picked.id }),
        kind: values.kind,
        qualifier: scope.qualifier || null,
        coverage: scope.coverage || null,
        from_range: values.from_range.trim() || null,
        to_range: values.to_range.trim() || null,
        note: values.note.trim() || null,
      },
      { onSuccess: onDone, onError },
    );
  });

  const rootError = form.formState.errors.root?.serverError?.message;

  return (
    <Form {...form}>
      <form onSubmit={onSubmit} className="space-y-4" noValidate>
        <FormField
          control={form.control}
          name="kind"
          render={({ field }) => (
            <FormItem>
              <FormLabel>Relationship</FormLabel>
              <FormControl>
                <RelationshipKindSelect
                  value={field.value}
                  onChange={onKind}
                  filter={
                    edit?.isArc
                      ? (k) =>
                          kindInfo(catalogue.data, k)?.allows_arc_target ??
                          false
                      : undefined
                  }
                />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />

        {!edit && (
          <div className="space-y-2">
            {/* Segmented "Series | Story arc" toggle on Radix RadioGroup:
                roving focus + arrow keys, a ring-token focus ring. */}
            <RadioGroupPrimitive.Root
              aria-label="Target"
              orientation="horizontal"
              value={targetType === "arc" && !allowsArc ? "series" : targetType}
              onValueChange={(v) => {
                form.setValue(
                  "target_type",
                  v as "series" | "arc" | "external",
                );
                form.clearErrors(["target", "target_arc"]);
              }}
              className="bg-muted text-muted-foreground inline-flex h-9 items-center rounded-md p-1 text-sm"
            >
              {(["series", "arc", "external"] as const).map((t) => {
                const disabled = t === "arc" && !allowsArc;
                return (
                  <RadioGroupPrimitive.Item
                    key={t}
                    value={t}
                    disabled={disabled}
                    title={
                      disabled
                        ? "Only “Tie-in to” can target a story arc"
                        : undefined
                    }
                    className="ring-offset-background focus-visible:ring-ring data-[state=checked]:bg-background data-[state=checked]:text-foreground rounded-sm px-3 py-1 font-medium transition-colors focus-visible:ring-2 focus-visible:ring-offset-2 focus-visible:outline-none disabled:cursor-not-allowed disabled:opacity-50 data-[state=checked]:shadow"
                  >
                    {t === "series"
                      ? "Series"
                      : t === "arc"
                        ? "Story arc"
                        : "Not in library"}
                  </RadioGroupPrimitive.Item>
                );
              })}
            </RadioGroupPrimitive.Root>
            {isExternal ? (
              <ExternalTargetFields />
            ) : targetType === "arc" && allowsArc ? (
              <FormField
                control={form.control}
                name="target_arc"
                render={({ field }) => (
                  <FormItem>
                    <FormLabel>Story arc</FormLabel>
                    <FormControl>
                      <TargetPicker
                        kind="arc"
                        value={field.value}
                        onChange={(v) => {
                          field.onChange(v);
                          form.clearErrors("target_arc");
                        }}
                      />
                    </FormControl>
                    <FormMessage />
                  </FormItem>
                )}
              />
            ) : (
              <FormField
                control={form.control}
                name="target"
                render={({ field }) => (
                  <FormItem>
                    <FormLabel>Series</FormLabel>
                    <FormControl>
                      <TargetPicker
                        kind="series"
                        excludeId={seriesId}
                        value={field.value}
                        onChange={(v) => {
                          field.onChange(v);
                          form.clearErrors("target");
                        }}
                      />
                    </FormControl>
                    <FormMessage />
                  </FormItem>
                )}
              />
            )}
          </div>
        )}

        {(qualifiers.length > 0 || (allowsCoverage && !isExternal)) && (
          <div className="grid gap-4 sm:grid-cols-2">
            {qualifiers.length > 0 && (
              <FormField
                control={form.control}
                name="qualifier"
                render={({ field }) => (
                  <FormItem>
                    <FormLabel>{isTieIn ? "Role" : "Qualifier"}</FormLabel>
                    <Select
                      value={field.value || "none"}
                      onValueChange={(v) =>
                        field.onChange(
                          v === "none" ? "" : (v as RelationshipQualifier),
                        )
                      }
                    >
                      <FormControl>
                        <SelectTrigger>
                          <SelectValue />
                        </SelectTrigger>
                      </FormControl>
                      <SelectContent>
                        <SelectItem value="none">None</SelectItem>
                        {qualifiers.map((q) => (
                          <SelectItem key={q.value} value={q.value}>
                            {q.label}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                    <FormMessage />
                  </FormItem>
                )}
              />
            )}
            {allowsCoverage && !isExternal && (
              <FormField
                control={form.control}
                name="coverage"
                render={({ field }) => (
                  <FormItem>
                    <FormLabel>Coverage</FormLabel>
                    <Select
                      value={field.value || "none"}
                      onValueChange={(v) =>
                        field.onChange(
                          v === "none" ? "" : (v as RelationshipCoverage),
                        )
                      }
                    >
                      <FormControl>
                        <SelectTrigger>
                          <SelectValue />
                        </SelectTrigger>
                      </FormControl>
                      <SelectContent>
                        <SelectItem value="none">Not set</SelectItem>
                        <SelectItem value="full">Full</SelectItem>
                        <SelectItem value="partial">Partial</SelectItem>
                        <SelectItem value="unknown">Unknown</SelectItem>
                      </SelectContent>
                    </Select>
                    <FormMessage />
                  </FormItem>
                )}
              />
            )}
          </div>
        )}

        {!isExternal && (
          <div className="grid gap-4 sm:grid-cols-2">
            <FormField
              control={form.control}
              name="from_range"
              render={({ field }) => (
                <FormItem>
                  <FormLabel>This series&rsquo; issues</FormLabel>
                  <FormControl>
                    <Input {...field} maxLength={100} placeholder="e.g. 1-6" />
                  </FormControl>
                  <FormMessage />
                </FormItem>
              )}
            />
            <FormField
              control={form.control}
              name="to_range"
              render={({ field }) => (
                <FormItem>
                  <FormLabel>
                    {targetType === "arc" ? "Arc parts" : "Their issues"}
                  </FormLabel>
                  <FormControl>
                    <Input
                      {...field}
                      maxLength={100}
                      placeholder="e.g. 1-6,Annual 1"
                    />
                  </FormControl>
                  <FormMessage />
                </FormItem>
              )}
            />
          </div>
        )}

        {!isExternal && (
          <FormField
            control={form.control}
            name="note"
            render={({ field }) => (
              <FormItem>
                <FormLabel>Note</FormLabel>
                <FormControl>
                  <Input {...field} maxLength={500} placeholder="Optional" />
                </FormControl>
                <FormMessage />
              </FormItem>
            )}
          />
        )}

        {rootError ? (
          <p role="alert" className="text-destructive text-sm">
            {rootError}
          </p>
        ) : null}

        <DialogFooter>
          <Button
            type="button"
            variant="ghost"
            onClick={onDone}
            disabled={pending}
          >
            Cancel
          </Button>
          <Button
            type="submit"
            disabled={pending || (!!edit && !form.formState.isDirty)}
          >
            {pending ? (
              <>
                <Loader2 className="mr-1 h-3 w-3 animate-spin" /> Saving
              </>
            ) : edit ? (
              "Save"
            ) : (
              "Add"
            )}
          </Button>
        </DialogFooter>
      </form>
    </Form>
  );
}

/** WP-7.8 "Not in library" target: which provider series this series
 *  relates to. Provider + numeric id identify it (the link back and, once
 *  the series is scanned in and matched, the promotion to an ordinary
 *  relationship); name and year are what the Related tab shows. */
function ExternalTargetFields() {
  const { control } = useFormContext<RelationshipFormValues>();
  return (
    <div className="space-y-3">
      <p className="text-muted-foreground text-xs">
        A series you don&rsquo;t have. It shows in the Related tab as &ldquo;not
        in your library&rdquo; with a link to the provider, and becomes a normal
        relationship once that series is added and matched.
      </p>
      <div className="grid gap-4 sm:grid-cols-[minmax(0,10rem)_minmax(0,1fr)]">
        <FormField
          control={control}
          name="source"
          render={({ field }) => (
            <FormItem>
              <FormLabel>Provider</FormLabel>
              <Select value={field.value} onValueChange={field.onChange}>
                <FormControl>
                  <SelectTrigger aria-label="Provider">
                    <SelectValue />
                  </SelectTrigger>
                </FormControl>
                <SelectContent>
                  {EXTERNAL_SOURCES.map((s) => (
                    <SelectItem key={s.value} value={s.value}>
                      {s.label}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              <FormMessage />
            </FormItem>
          )}
        />
        <FormField
          control={control}
          name="provider_series_id"
          render={({ field }) => (
            <FormItem>
              <FormLabel>Provider series id</FormLabel>
              <FormControl>
                <Input
                  {...field}
                  inputMode="numeric"
                  maxLength={12}
                  placeholder="e.g. 2311"
                />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />
      </div>
      <div className="grid gap-4 sm:grid-cols-[minmax(0,1fr)_minmax(0,7rem)]">
        <FormField
          control={control}
          name="name"
          render={({ field }) => (
            <FormItem>
              <FormLabel>Series name</FormLabel>
              <FormControl>
                <Input {...field} maxLength={300} placeholder="e.g. Saga" />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />
        <FormField
          control={control}
          name="year"
          render={({ field }) => (
            <FormItem>
              <FormLabel>Year</FormLabel>
              <FormControl>
                <Input
                  {...field}
                  inputMode="numeric"
                  maxLength={4}
                  placeholder="Optional"
                />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />
      </div>
    </div>
  );
}

function useDebounced<T>(value: T, ms: number): T {
  const [v, setV] = React.useState(value);
  React.useEffect(() => {
    const t = setTimeout(() => setV(value), ms);
    return () => clearTimeout(t);
  }, [value, ms]);
  return v;
}

/** Result list height: the room Radix reports for the popover minus the
 *  search row, capped at 18rem (the old fixed `max-h-72`). */
const TARGET_LIST_MAX_H =
  "max-h-[min(18rem,calc(var(--radix-popover-content-available-height,18rem)-2.75rem))]";

/** Typeahead over `/series?q=` or `/arcs?q=` (both cursor-paginated;
 *  "More results" walks the next page instead of silently capping). */
export function TargetPicker({
  kind,
  excludeId,
  value,
  onChange,
}: {
  kind: "series" | "arc";
  excludeId?: string;
  value: TargetRef | null;
  onChange: (t: TargetRef) => void;
}) {
  const [open, setOpen] = React.useState(false);
  const [text, setText] = React.useState("");
  const q = useDebounced(text.trim(), 200);
  const enabled = open && q.length > 0;
  const series = useSeriesListInfinite(
    { q, limit: 20 },
    { enabled: enabled && kind === "series" },
  );
  const arcs = useEntityListInfinite(
    "arcs",
    { q, limit: 20 },
    { enabled: enabled && kind === "arc" },
  );
  const search = kind === "series" ? series : arcs;
  const items: TargetRef[] =
    kind === "series"
      ? (series.data?.pages ?? [])
          .flatMap((p) => p.items)
          .filter((s) => s.id !== excludeId)
          .map((s) => ({
            id: s.id,
            name: s.year ? `${s.name} (${s.year})` : s.name,
            detail: s.publisher,
          }))
      : (arcs.data?.pages ?? [])
          .flatMap((p) => p.items)
          .map((a) => ({
            id: a.id,
            name: a.name,
            detail: `${a.series_count} series`,
          }));
  const noun = kind === "series" ? "series" : "story arc";

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button
          type="button"
          variant="outline"
          className="w-full justify-start font-normal"
          aria-label={value ? `${noun}: ${value.name}` : `Choose a ${noun}`}
        >
          {value ? (
            <span className="truncate">{value.name}</span>
          ) : (
            <span className="text-muted-foreground">Choose a {noun}…</span>
          )}
        </Button>
      </PopoverTrigger>
      <PopoverContent
        align="start"
        // One scroller only (the themed ScrollArea below), like the kind
        // picker: the popover itself never scrolls.
        className="w-[min(380px,calc(100vw-2rem))] overflow-hidden p-0"
      >
        {/* cmdk for arrow-key / Enter navigation and the highlighted-row
            styling; results are server-filtered, so cmdk's own filter is
            off. */}
        <Command shouldFilter={false}>
          <CommandInput
            value={text}
            onValueChange={setText}
            placeholder={`Search ${kind === "series" ? "series" : "story arcs"}…`}
            aria-label={`Search ${kind === "series" ? "series" : "story arcs"}`}
          />
          <ScrollArea
            type="auto"
            viewportClassName={TARGET_LIST_MAX_H}
            data-testid="relationship-target-scroll"
          >
            {/* `pr-2.5` keeps highlighted rows clear of the overlay
                scrollbar (`w-2.5`). */}
            <CommandList className="max-h-none overflow-visible pr-2.5">
              {q.length === 0 ? (
                <p className="text-muted-foreground px-3 py-6 text-center text-sm">
                  Type a name to search.
                </p>
              ) : search.isLoading ? (
                <p className="text-muted-foreground flex items-center justify-center gap-2 px-3 py-6 text-sm">
                  <Loader2 className="h-3.5 w-3.5 animate-spin" /> Searching…
                </p>
              ) : items.length === 0 ? (
                <p className="text-muted-foreground px-3 py-6 text-center text-sm">
                  No {noun} matched.
                </p>
              ) : (
                <CommandGroup>
                  {items.map((t) => (
                    <CommandItem
                      key={t.id}
                      value={t.id}
                      onSelect={() => {
                        onChange(t);
                        setOpen(false);
                      }}
                      // The publisher / count line follows the highlighted
                      // row's foreground (muted on accent is unreadable).
                      className="data-[selected=true]:*:text-accent-foreground flex-col items-start gap-0.5"
                    >
                      <span className="font-medium">{t.name}</span>
                      {t.detail ? (
                        <span className="text-muted-foreground text-xs">
                          {t.detail}
                        </span>
                      ) : null}
                    </CommandItem>
                  ))}
                </CommandGroup>
              )}
              {search.hasNextPage && (
                <div className="border-border border-t p-1">
                  <Button
                    type="button"
                    variant="ghost"
                    size="sm"
                    className="w-full"
                    disabled={search.isFetchingNextPage}
                    onClick={() => void search.fetchNextPage()}
                  >
                    {search.isFetchingNextPage ? "Loading…" : "More results"}
                  </Button>
                </div>
              )}
            </CommandList>
          </ScrollArea>
        </Command>
      </PopoverContent>
    </Popover>
  );
}
