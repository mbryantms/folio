"use client";

/**
 * `<SeriesRelatedSection>` — WP-7.1 "Related" block on the series page.
 *
 * Two parts, both from `GET /series/{slug}/relationships`:
 *  - **Reading order** — the sequel/prequel chain through this series
 *    (server-side recursive CTE, depth ≤ 6 each way), rendered as a
 *    horizontal strip with this series highlighted.
 *  - **Related series** — direct relationships grouped by UI group
 *    (Story · Publication history · Editions & contents · Advanced, WP-7.5)
 *    and kind label ("Sequel to", "Collected in", …), with ranges /
 *    coverage / qualifier / note as secondary text, plus tie-ins to story
 *    arcs.
 *
 * Everyone who can see the series sees the block (the API already drops
 * series the viewer can't see); admins also get add / remove and the
 * WP-7.3 "Suggested" chips (pending engine suggestions, one-click accept /
 * reject). Renders nothing when there are no relationships and the viewer
 * can't edit.
 */

import { ChevronRight, Loader2, Plus, Trash2 } from "lucide-react";
import Link from "next/link";
import * as React from "react";

import { Cover } from "@/components/Cover";
import { SeriesSuggestedRelationships } from "@/components/library/SeriesSuggestedRelationships";
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
  useRelationshipKinds,
  useSeriesListInfinite,
  useSeriesRelationships,
} from "@/lib/api/queries";
import { RelationshipKindSelect } from "@/components/library/RelationshipKindSelect";
import type {
  RelationshipCatalogue,
  RelationshipCoverage,
  RelationshipGroup,
  RelationshipKind,
  RelationshipQualifier,
  SeriesArcRelationshipView,
  SeriesChainEntry,
  SeriesRelationshipView,
  SeriesView,
} from "@/lib/api/types";
import { kindInfo, kindOrder, scopeCaption } from "@/lib/relationships";
import { seriesUrl } from "@/lib/urls";
import { cn } from "@/lib/utils";

/** Fallback group order until the catalogue loads (the catalogue's own
 *  `groups` order wins once it does). */
const DEFAULT_GROUP_ORDER: RelationshipGroup[] = [
  "story",
  "publication",
  "editions",
  "advanced",
];

/** Group direct relationships by UI group, then by display label (a
 *  tie-in role gives "Prelude to" its own heading), in catalogue order. */
export function groupRelationships(
  rels: SeriesRelationshipView[],
  catalogue?: RelationshipCatalogue,
): Array<{
  group: RelationshipGroup;
  label: string;
  kinds: Array<{
    kind: RelationshipKind;
    label: string;
    items: SeriesRelationshipView[];
  }>;
}> {
  const order = catalogue?.groups.map((g) => g.group) ?? DEFAULT_GROUP_ORDER;
  const sorted = [...rels].sort(
    (a, b) => kindOrder(catalogue, a.kind) - kindOrder(catalogue, b.kind),
  );
  return order
    .map((group) => {
      const kinds: Array<{
        kind: RelationshipKind;
        label: string;
        items: SeriesRelationshipView[];
      }> = [];
      for (const r of sorted.filter((r) => r.group === group)) {
        const slot = kinds.find((k) => k.label === r.kind_label);
        if (slot) slot.items.push(r);
        else kinds.push({ kind: r.kind, label: r.kind_label, items: [r] });
      }
      return {
        group,
        label:
          catalogue?.groups.find((g) => g.group === group)?.label ??
          group.charAt(0).toUpperCase() + group.slice(1),
        kinds,
      };
    })
    .filter((g) => g.kinds.length > 0);
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

  const catalogue = useRelationshipKinds();
  const rels = query.data?.relationships ?? [];
  const arcs = query.data?.arcs ?? [];
  const chain = query.data?.chain ?? [];
  const groups = groupRelationships(rels, catalogue.data);

  if (query.isLoading) return null;
  if (rels.length === 0 && arcs.length === 0 && chain.length === 0 && !isAdmin)
    return null;

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

      {isAdmin && (
        <SeriesSuggestedRelationships
          seriesSlug={seriesSlug}
          seriesId={seriesId}
        />
      )}

      {groups.length > 0 || arcs.length > 0 ? (
        <div className="space-y-5">
          {groups.map((g) => (
            <div key={g.group} className="space-y-3">
              <h3 className="text-foreground text-sm font-medium">{g.label}</h3>
              {g.kinds.map((k) => (
                <div key={k.label} className="space-y-2">
                  <h4 className="text-muted-foreground text-xs font-medium tracking-wider uppercase">
                    {k.label}
                  </h4>
                  <ul className="flex flex-wrap gap-3">
                    {k.items.map((r) => {
                      const caption = scopeCaption(r);
                      return (
                        <li key={r.id} className="relative">
                          <RelatedCard series={r.series}>
                            {r.source === "suggested" && (
                              <Badge
                                variant="secondary"
                                className="font-normal"
                              >
                                Suggested
                              </Badge>
                            )}
                            {caption && (
                              <p
                                className="text-muted-foreground line-clamp-3 text-xs"
                                title={caption}
                              >
                                {caption}
                              </p>
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
                      );
                    })}
                  </ul>
                </div>
              ))}
            </div>
          ))}
          {arcs.length > 0 && (
            <ArcLinks
              arcs={arcs}
              isAdmin={isAdmin}
              onRemove={(a) =>
                setConfirmRemove({
                  id: a.id,
                  otherSlug: seriesSlug,
                  label: `${a.kind_label} ${a.arc.name}`,
                })
              }
            />
          )}
        </div>
      ) : (
        isAdmin &&
        !adding && (
          <p className="text-muted-foreground text-sm">
            No related series yet. Link sequels, continuations, spin-offs and
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

/** Tie-ins to story arcs (WP-7.5): one-directional series → arc edges. */
function ArcLinks({
  arcs,
  isAdmin,
  onRemove,
}: {
  arcs: SeriesArcRelationshipView[];
  isAdmin: boolean;
  onRemove: (a: SeriesArcRelationshipView) => void;
}) {
  return (
    <div className="space-y-2">
      <h3 className="text-foreground text-sm font-medium">Story arcs</h3>
      <ul className="space-y-1 text-sm">
        {arcs.map((a) => {
          const caption = scopeCaption(a);
          return (
            <li key={a.id} className="flex items-center gap-2">
              <span className="text-muted-foreground">{a.kind_label}</span>
              <Link
                href={`/arcs/${encodeURIComponent(a.arc.slug)}`}
                className="font-medium hover:underline"
              >
                {a.arc.name}
              </Link>
              {caption && (
                <span className="text-muted-foreground text-xs">
                  · {caption}
                </span>
              )}
              {isAdmin && (
                <Button
                  variant="ghost"
                  size="icon"
                  className="text-muted-foreground hover:text-foreground h-6 w-6"
                  aria-label={`Remove ${a.kind_label.toLowerCase()} ${a.arc.name}`}
                  onClick={() => onRemove(a)}
                >
                  <Trash2 className="h-3.5 w-3.5" />
                </Button>
              )}
            </li>
          );
        })}
      </ul>
    </div>
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
  const catalogue = useRelationshipKinds();
  const [kind, setKind] = React.useState<RelationshipKind>("sequel_of");
  const [target, setTarget] = React.useState<SeriesView | null>(null);
  const [qualifier, setQualifier] = React.useState<RelationshipQualifier | "">(
    "",
  );
  const [coverage, setCoverage] = React.useState<RelationshipCoverage | "">("");
  const [fromRange, setFromRange] = React.useState("");
  const [toRange, setToRange] = React.useState("");
  const [note, setNote] = React.useState("");
  const info = kindInfo(catalogue.data, kind);
  const qualifiers = info?.qualifiers ?? [];
  const allowsCoverage = info?.allows_coverage ?? false;

  const onKind = (k: RelationshipKind) => {
    setKind(k);
    // Drop scope the new kind doesn't take (the server would 422).
    const next = kindInfo(catalogue.data, k);
    if (!next?.qualifiers.some((q) => q.value === qualifier)) setQualifier("");
    if (!next?.allows_coverage) setCoverage("");
  };

  const onSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    if (!target) return;
    create.mutate(
      {
        target: target.id,
        kind,
        qualifier: qualifier || null,
        coverage: coverage || null,
        from_range: fromRange.trim() || null,
        to_range: toRange.trim() || null,
        note: note.trim() || null,
      },
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
      className="border-border/60 space-y-3 border-t pt-3"
    >
      <div className="grid gap-3 sm:grid-cols-[14rem_1fr] sm:items-end">
        <div className="grid gap-1.5">
          <Label htmlFor="rel-kind" className="text-xs">
            Relationship
          </Label>
          <RelationshipKindSelect
            id="rel-kind"
            value={kind}
            onChange={onKind}
          />
        </div>
        <div className="grid gap-1.5">
          <Label className="text-xs">Series</Label>
          <SeriesTargetPicker
            excludeId={seriesId}
            value={target}
            onChange={setTarget}
          />
        </div>
      </div>
      <div className="grid gap-3 sm:grid-cols-4">
        {qualifiers.length > 0 && (
          <div className="grid gap-1.5">
            <Label htmlFor="rel-qualifier" className="text-xs">
              {kind === "tie_in_to" || kind === "has_tie_in"
                ? "Role"
                : "Qualifier"}
            </Label>
            <Select
              value={qualifier || "none"}
              onValueChange={(v) =>
                setQualifier(v === "none" ? "" : (v as RelationshipQualifier))
              }
            >
              <SelectTrigger id="rel-qualifier">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="none">None</SelectItem>
                {qualifiers.map((q) => (
                  <SelectItem key={q.value} value={q.value}>
                    {q.label}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
        )}
        {allowsCoverage && (
          <div className="grid gap-1.5">
            <Label htmlFor="rel-coverage" className="text-xs">
              Coverage
            </Label>
            <Select
              value={coverage || "none"}
              onValueChange={(v) =>
                setCoverage(v === "none" ? "" : (v as RelationshipCoverage))
              }
            >
              <SelectTrigger id="rel-coverage">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="none">Not set</SelectItem>
                <SelectItem value="full">Full</SelectItem>
                <SelectItem value="partial">Partial</SelectItem>
                <SelectItem value="unknown">Unknown</SelectItem>
              </SelectContent>
            </Select>
          </div>
        )}
        <div className="grid gap-1.5">
          <Label htmlFor="rel-from-range" className="text-xs">
            This series&rsquo; issues
          </Label>
          <Input
            id="rel-from-range"
            value={fromRange}
            maxLength={100}
            placeholder="e.g. 1-6"
            onChange={(e) => setFromRange(e.target.value)}
          />
        </div>
        <div className="grid gap-1.5">
          <Label htmlFor="rel-to-range" className="text-xs">
            Their issues
          </Label>
          <Input
            id="rel-to-range"
            value={toRange}
            maxLength={100}
            placeholder="e.g. 1-6,Annual 1"
            onChange={(e) => setToRange(e.target.value)}
          />
        </div>
      </div>
      <div className="grid gap-1.5">
        <Label htmlFor="rel-note" className="text-xs">
          Note
        </Label>
        <Input
          id="rel-note"
          value={note}
          maxLength={500}
          placeholder="Optional"
          onChange={(e) => setNote(e.target.value)}
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
