"use client";

/**
 * `<SeriesRelatedSection>` — the relationships part of the series page's
 * **Related** tab (WP-7.1, moved into the tab in WP-7.7). All from
 * `GET /series/{slug}/relationships`:
 *
 *  - **Reading order** — the chain through this series (`sequel_of` and
 *    `continues`, server-side recursive CTE, depth ≤ 6 each way), a
 *    horizontal strip with this series highlighted.
 *  - **Related series** — direct relationships grouped by UI group
 *    (Story · Publication history · Editions & contents · Advanced) and
 *    display label ("Sequel to", "Collected in", …), with ranges /
 *    coverage / qualifier / note as secondary text.
 *  - **Part of event** — the series → story-arc edges with their role
 *    ("Tie-in to Secret Wars", "Prelude to …").
 *
 * Covers render at `coverWidth` — the issue grid's effective column width
 * for the page's card-size slider (`useGridColumnWidth`), so every cover
 * in the tab matches the grid. Admins also get add / edit
 * (`RelationshipFormDialog`) / remove (behind an `AlertDialog`) and the
 * WP-7.3 "Suggested" chips.
 */

import { ChevronRight, Loader2, Pencil, Plus, Trash2 } from "lucide-react";
import Link from "next/link";
import * as React from "react";

import { Cover } from "@/components/Cover";
import {
  RelationshipFormDialog,
  type EditableRelationship,
} from "@/components/library/RelationshipFormDialog";
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
import { Skeleton } from "@/components/ui/skeleton";
import { useDeleteSeriesRelationship } from "@/lib/api/mutations";
import {
  useMe,
  useRelationshipKinds,
  useSeriesRelationships,
} from "@/lib/api/queries";
import type {
  RelationshipCatalogue,
  RelationshipGroup,
  RelationshipKind,
  SeriesArcRelationshipView,
  SeriesChainEntry,
  SeriesRelationshipView,
  SeriesView,
} from "@/lib/api/types";
import { SERIES_CARD_SIZE } from "@/lib/library/series-card-size";
import { kindOrder, scopeCaption } from "@/lib/relationships";
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

type RemoveTarget = {
  id: string;
  otherSlug: string;
  arcSlug?: string;
  label: string;
};

function editableFromSeries(r: SeriesRelationshipView): EditableRelationship {
  return {
    id: r.id,
    kind: r.kind,
    isArc: false,
    otherName:
      r.series.year != null
        ? `${r.series.name} (${r.series.year})`
        : r.series.name,
    qualifier: r.qualifier,
    coverage: r.coverage,
    from_range: r.from_range,
    to_range: r.to_range,
    note: r.note,
  };
}

function editableFromArc(a: SeriesArcRelationshipView): EditableRelationship {
  return {
    id: a.id,
    kind: a.kind,
    isArc: true,
    otherName: a.arc.name,
    qualifier: a.qualifier,
    from_range: a.from_range,
    to_range: a.to_range,
    note: a.note,
  };
}

export function SeriesRelatedSection({
  seriesSlug,
  seriesId,
  seriesName,
  coverWidth = SERIES_CARD_SIZE.defaultSize,
}: {
  seriesSlug: string;
  seriesId: string;
  seriesName?: string;
  /** Cover width in px (the issue grid's effective column width). */
  coverWidth?: number;
}) {
  const me = useMe();
  const isAdmin = me.data?.role === "admin";
  const query = useSeriesRelationships(seriesSlug);
  const remove = useDeleteSeriesRelationship(seriesSlug);
  const catalogue = useRelationshipKinds();
  const [formOpen, setFormOpen] = React.useState(false);
  const [editing, setEditing] = React.useState<EditableRelationship | null>(
    null,
  );
  const [confirmRemove, setConfirmRemove] = React.useState<RemoveTarget | null>(
    null,
  );

  const rels = query.data?.relationships ?? [];
  const arcs = query.data?.arcs ?? [];
  const chain = query.data?.chain ?? [];
  const groups = groupRelationships(rels, catalogue.data);

  if (query.isLoading) return <RelatedSkeleton coverWidth={coverWidth} />;
  if (query.isError) {
    return (
      <p className="text-muted-foreground text-sm">
        Couldn&apos;t load related series. Try refreshing.
      </p>
    );
  }
  const empty = rels.length === 0 && arcs.length === 0 && chain.length === 0;

  const openAdd = () => {
    setEditing(null);
    setFormOpen(true);
  };
  const openEdit = (e: EditableRelationship) => {
    setEditing(e);
    setFormOpen(true);
  };

  return (
    <section aria-labelledby="series-related-heading" className="space-y-6">
      <div className="flex items-center justify-between gap-2">
        <h2
          id="series-related-heading"
          className="text-lg font-semibold tracking-tight"
        >
          Related series
        </h2>
        {isAdmin && (
          <Button variant="outline" size="sm" onClick={openAdd}>
            <Plus className="mr-1 h-3.5 w-3.5" /> Add relationship
          </Button>
        )}
      </div>

      {chain.length > 1 && (
        <ReadingOrder
          chain={chain}
          currentId={seriesId}
          coverWidth={coverWidth}
        />
      )}

      {isAdmin && (
        <SeriesSuggestedRelationships
          seriesSlug={seriesSlug}
          seriesId={seriesId}
        />
      )}

      {groups.length > 0 && (
        <div className="space-y-6">
          {groups.map((g) => (
            <div key={g.group} className="space-y-3">
              {catalogue.data ? (
                <h3 className="text-foreground text-sm font-medium">
                  {g.label}
                </h3>
              ) : (
                <Skeleton
                  aria-label="Loading group"
                  className="h-5 w-32"
                  data-testid="relationship-group-loading"
                />
              )}
              {g.kinds.map((k) => (
                <div key={k.label} className="space-y-2">
                  <h4 className="text-muted-foreground text-xs font-medium tracking-wider uppercase">
                    {k.label}
                  </h4>
                  <ul className="flex flex-wrap gap-4">
                    {k.items.map((r) => {
                      const caption = scopeCaption(r);
                      return (
                        <li key={r.id} className="relative">
                          <RelatedCard series={r.series} width={coverWidth}>
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
                            <EditRemoveButtons
                              label={`${r.kind_label.toLowerCase()} ${r.series.name}`}
                              onEdit={() => openEdit(editableFromSeries(r))}
                              onRemove={() =>
                                setConfirmRemove({
                                  id: r.id,
                                  otherSlug: r.series.slug,
                                  label: `${r.kind_label} ${r.series.name}`,
                                })
                              }
                            />
                          )}
                        </li>
                      );
                    })}
                  </ul>
                </div>
              ))}
            </div>
          ))}
        </div>
      )}

      {arcs.length > 0 && (
        <PartOfEvent
          arcs={arcs}
          isAdmin={isAdmin}
          onEdit={(a) => openEdit(editableFromArc(a))}
          onRemove={(a) =>
            setConfirmRemove({
              id: a.id,
              otherSlug: seriesSlug,
              arcSlug: a.arc.slug,
              label: `${a.kind_label} ${a.arc.name}`,
            })
          }
        />
      )}

      {empty && (
        <p className="text-muted-foreground text-sm">
          {isAdmin
            ? "No related series yet. Link sequels, continuations, spin-offs, collected editions and event tie-ins so readers can follow the run."
            : "No related series linked yet."}
        </p>
      )}

      {isAdmin && (
        <RelationshipFormDialog
          open={formOpen}
          onOpenChange={(o) => {
            setFormOpen(o);
            if (!o) setEditing(null);
          }}
          seriesSlug={seriesSlug}
          seriesId={seriesId}
          seriesName={seriesName}
          edit={editing ?? undefined}
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
            <AlertDialogTitle>Remove relationship?</AlertDialogTitle>
            <AlertDialogDescription>
              Removes &ldquo;{confirmRemove?.label}&rdquo;
              {confirmRemove?.arcSlug
                ? ""
                : " and its reverse link on the other series"}
              .
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
                  {
                    id: confirmRemove.id,
                    otherSlug: confirmRemove.otherSlug,
                    arcSlug: confirmRemove.arcSlug,
                  },
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

function EditRemoveButtons({
  label,
  onEdit,
  onRemove,
  floating = true,
}: {
  label: string;
  onEdit: () => void;
  onRemove: () => void;
  floating?: boolean;
}) {
  return (
    <div
      className={cn(
        "flex gap-1",
        floating && "absolute top-1 right-1 rounded-md",
      )}
    >
      <Button
        variant="ghost"
        size="icon"
        className="bg-background/80 text-muted-foreground hover:text-foreground h-6 w-6"
        aria-label={`Edit ${label}`}
        onClick={onEdit}
      >
        <Pencil className="h-3.5 w-3.5" />
      </Button>
      <Button
        variant="ghost"
        size="icon"
        className="bg-background/80 text-muted-foreground hover:text-foreground h-6 w-6"
        aria-label={`Remove ${label}`}
        onClick={onRemove}
      >
        <Trash2 className="h-3.5 w-3.5" />
      </Button>
    </div>
  );
}

/** "Part of event" (WP-7.7): the series' story-arc edges, each read with
 *  its role ("Tie-in to Secret Wars", "Prelude to …"). */
function PartOfEvent({
  arcs,
  isAdmin,
  onEdit,
  onRemove,
}: {
  arcs: SeriesArcRelationshipView[];
  isAdmin: boolean;
  onEdit: (a: SeriesArcRelationshipView) => void;
  onRemove: (a: SeriesArcRelationshipView) => void;
}) {
  return (
    <div className="space-y-2" aria-labelledby="part-of-event-heading">
      <h3
        id="part-of-event-heading"
        className="text-foreground text-sm font-medium"
      >
        Part of event
      </h3>
      <ul className="space-y-1.5 text-sm">
        {arcs.map((a) => {
          const caption = scopeCaption(a);
          return (
            <li key={a.id} className="flex flex-wrap items-center gap-x-2">
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
              {a.source === "suggested" && (
                <Badge variant="secondary" className="font-normal">
                  Suggested
                </Badge>
              )}
              {isAdmin && (
                <EditRemoveButtons
                  floating={false}
                  label={`${a.kind_label.toLowerCase()} ${a.arc.name}`}
                  onEdit={() => onEdit(a)}
                  onRemove={() => onRemove(a)}
                />
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
  coverWidth,
}: {
  chain: SeriesChainEntry[];
  currentId: string;
  coverWidth: number;
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
                  className="text-muted-foreground h-4 w-4 shrink-0"
                  // Centre the chevron on the cover (2:3).
                  style={{ marginTop: coverWidth * 0.75 - 8 }}
                />
              )}
              <RelatedCard
                series={entry.series}
                width={coverWidth}
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
  width,
  highlight,
  caption,
  children,
}: {
  series: SeriesView;
  width: number;
  highlight?: boolean;
  caption?: string;
  children?: React.ReactNode;
}) {
  return (
    <Link
      href={seriesUrl(series)}
      className="group flex flex-col gap-1.5"
      style={{ width: `${width}px` }}
      data-testid="related-card"
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

/** Placeholder while the relationships load: a heading bar and one row of
 *  covers at the grid's width, so the tab doesn't jump when data lands. */
function RelatedSkeleton({ coverWidth }: { coverWidth: number }) {
  return (
    <div className="space-y-4" aria-busy="true" aria-label="Loading related">
      <Skeleton className="h-6 w-40" />
      <div className="flex gap-4 overflow-hidden">
        {Array.from({ length: 4 }).map((_, i) => (
          <div key={i} style={{ width: coverWidth }} className="shrink-0">
            <Skeleton className="aspect-[2/3] w-full" />
            <Skeleton className="mt-2 h-4 w-3/4" />
          </div>
        ))}
      </div>
    </div>
  );
}
