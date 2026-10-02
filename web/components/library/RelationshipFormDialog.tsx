"use client";

/**
 * `<RelationshipFormDialog>` — WP-7.7 add / edit form for a series
 * relationship (admin only), in a dialog so the Related tab never
 * reflows around an inline form.
 *
 * - **Add** (`mode="create"`): the grouped kind picker, a target toggle
 *   "Series | Story arc" (arc only for arc-capable kinds — `tie_in_to`)
 *   with a typeahead for each (`/series?q=` and `/arcs?q=`, both
 *   cursor-paginated with "More results"), then the scope the kind takes.
 *   `POST /series/{slug}/relationships`.
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

import { Loader2 } from "lucide-react";
import * as React from "react";
import { useForm, useWatch, type Path } from "react-hook-form";

import { RelationshipKindSelect } from "@/components/library/RelationshipKindSelect";
import { Button } from "@/components/ui/button";
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
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { applyServerErrors } from "@/lib/api/form-errors";
import {
  useCreateSeriesRelationship,
  useUpdateSeriesRelationship,
} from "@/lib/api/mutations";
import {
  useEntityListInfinite,
  useRelationshipKinds,
  useSeriesListInfinite,
} from "@/lib/api/queries";
import type {
  RelationshipCatalogue,
  RelationshipCoverage,
  RelationshipKind,
  RelationshipQualifier,
} from "@/lib/api/types";
import { kindInfo } from "@/lib/relationships";
import { cn } from "@/lib/utils";

/** A picked target: a series or a story arc (id + display name). */
export type TargetRef = { id: string; name: string; detail?: string | null };

export type RelationshipFormValues = {
  kind: RelationshipKind;
  target_type: "series" | "arc";
  target: TargetRef | null;
  target_arc: TargetRef | null;
  qualifier: RelationshipQualifier | "";
  coverage: RelationshipCoverage | "";
  from_range: string;
  to_range: string;
  note: string;
};

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
  };
}

export function RelationshipFormDialog({
  open,
  onOpenChange,
  seriesSlug,
  seriesId,
  seriesName,
  edit,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  seriesSlug: string;
  seriesId: string;
  seriesName?: string;
  /** Set to edit an existing relationship; omit to add one. */
  edit?: EditableRelationship;
}) {
  const [portal, setPortal] = React.useState<HTMLElement | null>(null);
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        ref={setPortal}
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
              : "Link a series or story arc. The reverse link is added on the other series automatically."}
          </DialogDescription>
        </DialogHeader>
        <PopoverPortalContainer value={portal}>
          <div className="-mx-1 min-h-0 flex-1 overflow-y-auto px-1">
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
  const update = useUpdateSeriesRelationship(seriesSlug);
  const pending = create.isPending || update.isPending;
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

  const onKind = (k: RelationshipKind) => {
    form.setValue("kind", k, { shouldDirty: true });
    const scope = scopeForKind(catalogue.data, k, form.getValues());
    form.setValue("qualifier", scope.qualifier, { shouldDirty: true });
    form.setValue("coverage", scope.coverage, { shouldDirty: true });
    if (!kindInfo(catalogue.data, k)?.allows_arc_target) {
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
            <div
              role="radiogroup"
              aria-label="Target"
              className="bg-muted text-muted-foreground inline-flex h-9 items-center rounded-md p-1 text-sm"
            >
              {(["series", "arc"] as const).map((t) => {
                const disabled = t === "arc" && !allowsArc;
                const active = targetType === t && !disabled;
                return (
                  <button
                    key={t}
                    type="button"
                    role="radio"
                    aria-checked={active}
                    disabled={disabled}
                    title={
                      disabled
                        ? "Only “Tie-in to” can target a story arc"
                        : undefined
                    }
                    onClick={() => {
                      form.setValue("target_type", t);
                      form.clearErrors(["target", "target_arc"]);
                    }}
                    className={cn(
                      "rounded-sm px-3 py-1 font-medium transition-colors disabled:cursor-not-allowed disabled:opacity-50",
                      active && "bg-background text-foreground shadow",
                    )}
                  >
                    {t === "series" ? "Series" : "Story arc"}
                  </button>
                );
              })}
            </div>
            {targetType === "arc" && allowsArc ? (
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

        {(qualifiers.length > 0 || allowsCoverage) && (
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
            {allowsCoverage && (
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

function useDebounced<T>(value: T, ms: number): T {
  const [v, setV] = React.useState(value);
  React.useEffect(() => {
    const t = setTimeout(() => setV(value), ms);
    return () => clearTimeout(t);
  }, [value, ms]);
  return v;
}

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
        className="w-[min(380px,calc(100vw-2rem))] p-0"
        align="start"
      >
        <div className="border-border border-b p-2">
          <Input
            autoFocus
            value={text}
            onChange={(e) => setText(e.target.value)}
            placeholder={`Search ${kind === "series" ? "series" : "story arcs"}…`}
            aria-label={`Search ${kind === "series" ? "series" : "story arcs"}`}
          />
        </div>
        <div className="max-h-72 overflow-auto">
          {q.length === 0 ? (
            <p className="text-muted-foreground p-3 text-xs">
              Type a name to search.
            </p>
          ) : search.isLoading ? (
            <p className="text-muted-foreground flex items-center gap-2 p-3 text-xs">
              <Loader2 className="h-3 w-3 animate-spin" /> Searching…
            </p>
          ) : items.length === 0 ? (
            <p className="text-muted-foreground p-3 text-xs">
              No {noun} matched.
            </p>
          ) : (
            <ul className="divide-border divide-y">
              {items.map((t) => (
                <li key={t.id}>
                  <button
                    type="button"
                    className="hover:bg-accent flex w-full flex-col gap-0.5 px-3 py-2 text-left text-sm"
                    onClick={() => {
                      onChange(t);
                      setOpen(false);
                    }}
                  >
                    <span className="font-medium">{t.name}</span>
                    {t.detail ? (
                      <span className="text-muted-foreground text-xs">
                        {t.detail}
                      </span>
                    ) : null}
                  </button>
                </li>
              ))}
            </ul>
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
        </div>
      </PopoverContent>
    </Popover>
  );
}
