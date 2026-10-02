"use client";

/**
 * `<SeriesRelatedSection>` — WP-7.1 "Related" block on the series page.
 *
 * Two parts, both from `GET /series/{slug}/relationships`:
 *  - **Reading order** — the sequel/prequel chain through this series
 *    (server-side recursive CTE, depth ≤ 6 each way), rendered as a
 *    horizontal strip with this series highlighted.
 *  - **Related series** — direct relationships grouped by kind
 *    ("Sequel of", "Collected in", …).
 *
 * Everyone who can see the series sees the block (the API already drops
 * series the viewer can't see); admins also get add / remove. Renders
 * nothing when there are no relationships and the viewer can't edit.
 */

import { ChevronRight, Loader2, Plus, Trash2 } from "lucide-react";
import Link from "next/link";
import * as React from "react";

import { Cover } from "@/components/Cover";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
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
import {
  useCreateSeriesRelationship,
  useDeleteSeriesRelationship,
} from "@/lib/api/mutations";
import {
  useMe,
  useSeriesListInfinite,
  useSeriesRelationships,
} from "@/lib/api/queries";
import type {
  RelationshipKind,
  SeriesChainEntry,
  SeriesRelationshipView,
  SeriesView,
} from "@/lib/api/types";
import { seriesUrl } from "@/lib/urls";
import { cn } from "@/lib/utils";

/** Kinds in the order the add-form offers them, with the sentence the
 *  admin is completing ("This series is a sequel of …"). */
export const RELATIONSHIP_KINDS: Array<{
  value: RelationshipKind;
  label: string;
}> = [
  { value: "sequel_of", label: "Sequel of" },
  { value: "prequel_of", label: "Prequel of" },
  { value: "spin_off_of", label: "Spin-off of" },
  { value: "has_spin_off", label: "Has spin-off" },
  { value: "crossover_with", label: "Crossover with" },
  { value: "collects", label: "Collects" },
  { value: "collected_in", label: "Collected in" },
  { value: "same_universe", label: "Same universe as" },
  { value: "see_also", label: "See also" },
];

/** Group direct relationships by kind, keeping the add-form's kind order. */
export function groupRelationships(
  rels: SeriesRelationshipView[],
): Array<{
  kind: RelationshipKind;
  label: string;
  items: SeriesRelationshipView[];
}> {
  return RELATIONSHIP_KINDS.map(({ value }) => {
    const items = rels.filter((r) => r.kind === value);
    return {
      kind: value,
      label: items[0]?.kind_label ?? value,
      items,
    };
  }).filter((g) => g.items.length > 0);
}

/** "Read before" / "This series" / "Read after" caption for a chain slot. */
export function chainCaption(position: number): string {
  if (position === 0) return "This series";
  return position < 0 ? "Read before" : "Read after";
}

type RemoveTarget = { id: string; otherSlug: string; label: string };

export function SeriesRelatedSection({
  seriesSlug,
  seriesId,
}: {
  seriesSlug: string;
  seriesId: string;
}) {
  const me = useMe();
  const isAdmin = me.data?.role === "admin";
  const query = useSeriesRelationships(seriesSlug);
  const remove = useDeleteSeriesRelationship(seriesSlug);
  const [adding, setAdding] = React.useState(false);
  const [confirmRemove, setConfirmRemove] = React.useState<RemoveTarget | null>(
    null,
  );

  const rels = query.data?.relationships ?? [];
  const chain = query.data?.chain ?? [];
  const groups = groupRelationships(rels);

  if (query.isLoading) return null;
  if (rels.length === 0 && chain.length === 0 && !isAdmin) return null;

  return (
    <section aria-labelledby="series-related-heading" className="space-y-4">
      <div className="flex items-center justify-between gap-2">
        <h2
          id="series-related-heading"
          className="text-foreground text-base font-semibold"
        >
          Related
        </h2>
        {isAdmin && !adding && (
          <Button variant="ghost" size="sm" onClick={() => setAdding(true)}>
            <Plus className="mr-1 h-3.5 w-3.5" /> Add related series
          </Button>
        )}
      </div>

      {chain.length > 1 && <ReadingOrder chain={chain} currentId={seriesId} />}

      {groups.length > 0 ? (
        <div className="space-y-4">
          {groups.map((g) => (
            <div key={g.kind} className="space-y-2">
              <h3 className="text-muted-foreground text-xs font-medium tracking-wider uppercase">
                {g.label}
              </h3>
              <ul className="flex flex-wrap gap-3">
                {g.items.map((r) => (
                  <li key={r.id} className="relative">
                    <RelatedCard series={r.series}>
                      {r.source === "suggested" && (
                        <Badge variant="secondary" className="font-normal">
                          Suggested
                        </Badge>
                      )}
                    </RelatedCard>
                    {isAdmin && (
                      <Button
                        variant="ghost"
                        size="icon"
                        className="bg-background/80 text-muted-foreground hover:text-foreground absolute top-1 right-1 h-6 w-6"
                        aria-label={`Remove ${r.kind_label.toLowerCase()} ${r.series.name}`}
                        onClick={() =>
                          setConfirmRemove({
                            id: r.id,
                            otherSlug: r.series.slug,
                            label: `${r.kind_label} ${r.series.name}`,
                          })
                        }
                      >
                        <Trash2 className="h-3.5 w-3.5" />
                      </Button>
                    )}
                  </li>
                ))}
              </ul>
            </div>
          ))}
        </div>
      ) : (
        isAdmin &&
        !adding && (
          <p className="text-muted-foreground text-sm">
            No related series yet. Link sequels, prequels, spin-offs and
            collected editions so readers can follow the run.
          </p>
        )
      )}

      {isAdmin && adding && (
        <AddRelationshipForm
          seriesSlug={seriesSlug}
          seriesId={seriesId}
          onDone={() => setAdding(false)}
        />
      )}

      <AlertDialog
        open={confirmRemove !== null}
        onOpenChange={(o) => {
          if (!o) setConfirmRemove(null);
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Remove related series?</AlertDialogTitle>
            <AlertDialogDescription>
              Removes &ldquo;{confirmRemove?.label}&rdquo; and its reverse link
              on the other series.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={remove.isPending}>
              Cancel
            </AlertDialogCancel>
            <AlertDialogAction
              disabled={remove.isPending}
              onClick={() => {
                if (!confirmRemove) return;
                remove.mutate(
                  { id: confirmRemove.id, otherSlug: confirmRemove.otherSlug },
                  { onSuccess: () => setConfirmRemove(null) },
                );
              }}
            >
              {remove.isPending ? (
                <>
                  <Loader2 className="mr-1 h-3 w-3 animate-spin" /> Removing
                </>
              ) : (
                "Remove"
              )}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </section>
  );
}

function ReadingOrder({
  chain,
  currentId,
}: {
  chain: SeriesChainEntry[];
  currentId: string;
}) {
  return (
    <div className="space-y-2">
      <h3 className="text-muted-foreground text-xs font-medium tracking-wider uppercase">
        Reading order
      </h3>
      <ol
        aria-label="Reading order"
        className="flex items-start gap-1 overflow-x-auto pb-1"
      >
        {chain.map((entry, i) => {
          const current = entry.series.id === currentId;
          return (
            <li
              key={entry.series.id}
              className="flex shrink-0 items-start gap-1"
              aria-current={current ? "true" : undefined}
            >
              {i > 0 && (
                <ChevronRight
                  aria-hidden
                  className="text-muted-foreground mt-14 h-4 w-4 shrink-0"
                />
              )}
              <RelatedCard
                series={entry.series}
                highlight={current}
                caption={chainCaption(entry.position)}
              />
            </li>
          );
        })}
      </ol>
    </div>
  );
}

function RelatedCard({
  series,
  highlight,
  caption,
  children,
}: {
  series: SeriesView;
  highlight?: boolean;
  caption?: string;
  children?: React.ReactNode;
}) {
  return (
    <Link
      href={seriesUrl(series)}
      className="group flex w-24 flex-col gap-1.5 sm:w-28"
    >
      <Cover
        src={series.cover_url}
        alt={series.name}
        fallback={series.publisher}
        className={cn(
          "transition-opacity group-hover:opacity-90",
          highlight && "ring-primary ring-2",
        )}
      />
      <div className="space-y-0.5 px-0.5">
        {caption && (
          <p
            className={cn(
              "text-[10px] font-medium tracking-wider uppercase",
              highlight ? "text-primary" : "text-muted-foreground",
            )}
          >
            {caption}
          </p>
        )}
        <p className="line-clamp-2 text-sm leading-tight font-medium group-hover:underline">
          {series.name}
        </p>
        {series.year != null && (
          <p className="text-muted-foreground text-xs">{series.year}</p>
        )}
        {children}
      </div>
    </Link>
  );
}

function AddRelationshipForm({
  seriesSlug,
  seriesId,
  onDone,
}: {
  seriesSlug: string;
  seriesId: string;
  onDone: () => void;
}) {
  const create = useCreateSeriesRelationship(seriesSlug);
  const [kind, setKind] = React.useState<RelationshipKind>("sequel_of");
  const [target, setTarget] = React.useState<SeriesView | null>(null);

  const onSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    if (!target) return;
    create.mutate(
      { target: target.id, kind },
      {
        onSuccess: () => {
          setTarget(null);
          onDone();
        },
      },
    );
  };

  return (
    <form
      onSubmit={onSubmit}
      className="border-border/60 grid gap-3 border-t pt-3 sm:grid-cols-[12rem_1fr_auto] sm:items-end"
    >
      <div className="grid gap-1.5">
        <Label htmlFor="rel-kind" className="text-xs">
          This series is…
        </Label>
        <Select
          value={kind}
          onValueChange={(v) => setKind(v as RelationshipKind)}
        >
          <SelectTrigger id="rel-kind">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {RELATIONSHIP_KINDS.map((k) => (
              <SelectItem key={k.value} value={k.value}>
                {k.label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>
      <div className="grid gap-1.5">
        <Label className="text-xs">Series</Label>
        <SeriesTargetPicker
          excludeId={seriesId}
          value={target}
          onChange={setTarget}
        />
      </div>
      <div className="flex gap-1">
        <Button type="submit" size="sm" disabled={!target || create.isPending}>
          {create.isPending ? (
            <>
              <Loader2 className="mr-1 h-3 w-3 animate-spin" /> Adding
            </>
          ) : (
            "Add"
          )}
        </Button>
        <Button
          type="button"
          size="sm"
          variant="ghost"
          onClick={onDone}
          disabled={create.isPending}
        >
          Cancel
        </Button>
      </div>
    </form>
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

/** Typeahead over `/series?q=` (cursor-paginated; "More results" walks
 *  the next page instead of silently capping). */
function SeriesTargetPicker({
  excludeId,
  value,
  onChange,
}: {
  excludeId: string;
  value: SeriesView | null;
  onChange: (s: SeriesView) => void;
}) {
  const [open, setOpen] = React.useState(false);
  const [text, setText] = React.useState("");
  const q = useDebounced(text.trim(), 200);
  const search = useSeriesListInfinite(
    { q, limit: 20 },
    { enabled: open && q.length > 0 },
  );
  const items = (search.data?.pages ?? [])
    .flatMap((p) => p.items)
    .filter((s) => s.id !== excludeId);

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button
          type="button"
          variant="outline"
          className="justify-start font-normal"
        >
          {value ? (
            <span className="truncate">
              {value.name}
              {value.year ? (
                <span className="text-muted-foreground"> · {value.year}</span>
              ) : null}
            </span>
          ) : (
            <span className="text-muted-foreground">Choose a series…</span>
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
            placeholder="Search series…"
            aria-label="Search series"
          />
        </div>
        <div className="max-h-72 overflow-auto">
          {q.length === 0 ? (
            <p className="text-muted-foreground p-3 text-xs">
              Type a series name to search.
            </p>
          ) : search.isLoading ? (
            <p className="text-muted-foreground flex items-center gap-2 p-3 text-xs">
              <Loader2 className="h-3 w-3 animate-spin" /> Searching…
            </p>
          ) : items.length === 0 ? (
            <p className="text-muted-foreground p-3 text-xs">
              No series matched.
            </p>
          ) : (
            <ul className="divide-border divide-y">
              {items.map((s) => (
                <li key={s.id}>
                  <button
                    type="button"
                    className="hover:bg-accent flex w-full flex-col gap-0.5 px-3 py-2 text-left text-sm"
                    onClick={() => {
                      onChange(s);
                      setOpen(false);
                    }}
                  >
                    <span className="font-medium">
                      {s.name}
                      {s.year ? (
                        <span className="text-muted-foreground">
                          {" "}
                          · {s.year}
                        </span>
                      ) : null}
                    </span>
                    {s.publisher && (
                      <span className="text-muted-foreground text-xs">
                        {s.publisher}
                      </span>
                    )}
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
