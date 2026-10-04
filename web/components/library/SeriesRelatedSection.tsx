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
 *  - **Not in your library** (WP-7.8) — links to provider series the
 *    library doesn't have (Metron `associated`, or added by an admin),
 *    listed in their kind's group as compact, muted text rows with a
 *    provider link ("Continued by: Saga (2018) — not in your library").
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
import { HorizontalScrollRail } from "@/components/library/HorizontalScrollRail";
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
import { Badge, badgeVariants } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import {
  useDeleteExternalRelationship,
  useDeleteSeriesRelationship,
} from "@/lib/api/mutations";
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
  SeriesExternalRelationshipView,
  SeriesRelationshipView,
  SeriesView,
} from "@/lib/api/types";
import { SERIES_CARD_SIZE } from "@/lib/library/series-card-size";
import { kindOrder, scopeCaption } from "@/lib/relationships";
import { useReturnFocus } from "@/lib/ui/use-return-focus";
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
 *  tie-in role gives "Prelude to" its own heading), in catalogue order.
 *  WP-7.8: external (not-in-library) links join the slot of their kind
 *  label as `external`, so a group can hold only external rows. */
export function groupRelationships(
  rels: SeriesRelationshipView[],
  catalogue?: RelationshipCatalogue,
  externals: SeriesExternalRelationshipView[] = [],
): Array<{
  group: RelationshipGroup;
  label: string;
  kinds: Array<{
    kind: RelationshipKind;
    label: string;
    items: SeriesRelationshipView[];
    external: SeriesExternalRelationshipView[];
  }>;
}> {
  const order = catalogue?.groups.map((g) => g.group) ?? DEFAULT_GROUP_ORDER;
  type Row =
    { local: SeriesRelationshipView } | { ext: SeriesExternalRelationshipView };
  const kindOf = (r: Row) => ("local" in r ? r.local : r.ext);
  const rows: Row[] = [
    ...rels.map((local) => ({ local })),
    ...externals.map((ext) => ({ ext })),
  ].sort(
    (a, b) =>
      kindOrder(catalogue, kindOf(a).kind) -
      kindOrder(catalogue, kindOf(b).kind),
  );
  return order
    .map((group) => {
      const kinds: Array<{
        kind: RelationshipKind;
        label: string;
        items: SeriesRelationshipView[];
        external: SeriesExternalRelationshipView[];
      }> = [];
      for (const r of rows.filter((r) => kindOf(r).group === group)) {
        const v = kindOf(r);
        let slot = kinds.find((k) => k.label === v.kind_label);
        if (!slot) {
          slot = { kind: v.kind, label: v.kind_label, items: [], external: [] };
          kinds.push(slot);
        }
        if ("local" in r) slot.items.push(r.local);
        else slot.external.push(r.ext);
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
  const removeExternal = useDeleteExternalRelationship(seriesSlug);
  const catalogue = useRelationshipKinds();
  const [formOpen, setFormOpen] = React.useState(false);
  const [editing, setEditing] = React.useState<EditableRelationship | null>(
    null,
  );
  const [confirmRemove, setConfirmRemove] = React.useState<RemoveTarget | null>(
    null,
  );
  // The dialogs are controlled (opened from per-card buttons), so focus
  // has to be handed back to the opener explicitly on close.
  const returnFocus = useReturnFocus();
  const askRemove = (t: RemoveTarget) => {
    returnFocus.capture();
    setConfirmRemove(t);
  };
  const [confirmRemoveExternal, setConfirmRemoveExternal] =
    React.useState<SeriesExternalRelationshipView | null>(null);
  const askRemoveExternal = (row: SeriesExternalRelationshipView) => {
    returnFocus.capture();
    setConfirmRemoveExternal(row);
  };

  const rels = query.data?.relationships ?? [];
  const arcs = query.data?.arcs ?? [];
  const chain = query.data?.chain ?? [];
  const externals = query.data?.external ?? [];
  const groups = groupRelationships(rels, catalogue.data, externals);

  if (query.isLoading) return <RelatedSkeleton coverWidth={coverWidth} />;
  if (query.isError) {
    return (
      <p className="text-muted-foreground text-sm">
        Couldn&apos;t load related series. Try refreshing.
      </p>
    );
  }
  const empty =
    rels.length === 0 &&
    arcs.length === 0 &&
    chain.length === 0 &&
    externals.length === 0;

  const openAdd = () => {
    returnFocus.capture();
    setEditing(null);
    setFormOpen(true);
  };
  const openEdit = (e: EditableRelationship) => {
    returnFocus.capture();
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
                  {k.items.length > 0 && (
                    <ul className="flex flex-wrap gap-4">
                      {k.items.map((r) => {
                        const caption = scopeCaption(r);
                        return (
                          <li key={r.id} className="group/rel relative">
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
                                  askRemove({
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
                  )}
                  {k.external.length > 0 && (
                    <ExternalRows
                      rows={k.external}
                      isAdmin={isAdmin}
                      onRemove={askRemoveExternal}
                    />
                  )}
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
            askRemove({
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
          onCloseAutoFocus={returnFocus.onCloseAutoFocus}
        />
      )}

      <AlertDialog
        open={confirmRemove !== null}
        onOpenChange={(o) => {
          if (!o) setConfirmRemove(null);
        }}
      >
        <AlertDialogContent onCloseAutoFocus={returnFocus.onCloseAutoFocus}>
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
      <AlertDialog
        open={confirmRemoveExternal !== null}
        onOpenChange={(o) => {
          if (!o) setConfirmRemoveExternal(null);
        }}
      >
        <AlertDialogContent onCloseAutoFocus={returnFocus.onCloseAutoFocus}>
          <AlertDialogHeader>
            <AlertDialogTitle>Remove external link?</AlertDialogTitle>
            <AlertDialogDescription>
              Removes &ldquo;{confirmRemoveExternal?.kind_label}{" "}
              {confirmRemoveExternal ? externalName(confirmRemoveExternal) : ""}
              &rdquo;.
              {confirmRemoveExternal?.set_by === "provider"
                ? ` It came from ${confirmRemoveExternal.source_label}; a later metadata update won't bring it back.`
                : ""}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={removeExternal.isPending}>
              Cancel
            </AlertDialogCancel>
            <AlertDialogAction
              disabled={removeExternal.isPending}
              onClick={() => {
                if (!confirmRemoveExternal) return;
                removeExternal.mutate(
                  { id: confirmRemoveExternal.id },
                  { onSuccess: () => setConfirmRemoveExternal(null) },
                );
              }}
            >
              {removeExternal.isPending ? (
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

/** "Saga (2018)". */
export function externalName(e: SeriesExternalRelationshipView): string {
  return e.year != null ? `${e.name} (${e.year})` : e.name;
}

/** WP-7.8: links to provider series that aren't in the library — compact,
 *  muted text rows (no cover: there is nothing local to show), each with
 *  the provider attribution link. A user link whose target was matched
 *  locally since (not yet promoted by the next suggestion run) links to
 *  the local series instead. */
function ExternalRows({
  rows,
  isAdmin,
  onRemove,
}: {
  rows: SeriesExternalRelationshipView[];
  isAdmin: boolean;
  onRemove: (e: SeriesExternalRelationshipView) => void;
}) {
  return (
    <ul className="space-y-1 text-sm" aria-label="Not in your library">
      {rows.map((e) => (
        <li
          key={e.id}
          className="text-muted-foreground flex min-w-0 flex-wrap items-center gap-x-2 gap-y-0.5"
          data-testid="external-relationship"
        >
          <span className="text-foreground/80 min-w-0 font-medium break-words">
            {externalName(e)}
          </span>
          {e.local_series ? (
            <Link
              href={seriesUrl(e.local_series)}
              className="focus-visible:ring-ring rounded-sm text-xs underline-offset-2 hover:underline focus-visible:ring-2 focus-visible:outline-none"
            >
              in your library
            </Link>
          ) : (
            <span className="text-xs">— not in your library</span>
          )}
          {e.qualifier_label && (
            <span className="text-xs">· {e.qualifier_label}</span>
          )}
          {e.note && (
            <span className="text-xs" data-testid="external-relationship-note">
              · {e.note}
            </span>
          )}
          {e.url ? (
            <a
              href={e.url}
              target="_blank"
              rel="noreferrer"
              className={cn(
                badgeVariants({ variant: "outline" }),
                "text-muted-foreground hover:bg-muted hover:text-foreground focus-visible:ring-ring focus-visible:ring-offset-background font-medium transition-colors focus-visible:ring-2 focus-visible:ring-offset-2 focus-visible:outline-none",
              )}
              title={`${e.source_label} · ${e.provider_series_id}`}
            >
              {e.source_label}
            </a>
          ) : (
            <Badge
              variant="outline"
              className="text-muted-foreground font-medium"
            >
              {e.source_label}
            </Badge>
          )}
          {isAdmin && (
            <Button
              variant="ghost"
              size="icon"
              className="text-muted-foreground hover:text-foreground h-6 w-6"
              aria-label={`Remove ${e.kind_label.toLowerCase()} ${externalName(e)}`}
              onClick={() => onRemove(e)}
            >
              <Trash2 className="h-3.5 w-3.5" />
            </Button>
          )}
        </li>
      ))}
    </ul>
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
        // Over a cover: revealed on the card's hover / keyboard focus for
        // mouse users; always shown on touch (no hover to reveal it).
        floating &&
          "absolute top-1 right-1 rounded-md transition-opacity pointer-fine:opacity-0 pointer-fine:group-focus-within/rel:opacity-100 pointer-fine:group-hover/rel:opacity-100",
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
    <section className="space-y-3" aria-labelledby="part-of-event-heading">
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
                className="focus-visible:ring-ring rounded-sm font-medium hover:underline focus-visible:ring-2 focus-visible:outline-none"
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
    </section>
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
    <div className="space-y-3">
      <h3 className="text-foreground text-sm font-medium">Reading order</h3>
      {/* The shared rail (not a bare `overflow-x-auto` list): it keeps ring
          room around every card so the current series' highlight ring and
          focus rings aren't clipped, hides the native scrollbar like every
          other strip, and centres the current series on load. */}
      <HorizontalScrollRail
        as="ol"
        trackLabel="Reading order"
        trackClassName="items-start gap-1"
        anchorAlign="center"
      >
        {chain.map((entry, i) => {
          const current = entry.series.id === currentId;
          return (
            <li
              key={entry.series.id}
              className="flex shrink-0 items-start gap-1"
              aria-current={current ? "true" : undefined}
              data-rail-current={current ? "true" : undefined}
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
      </HorizontalScrollRail>
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
      // Focus ring sits outside the card (offset) so the cover keeps the
      // grid's exact width; every container of these cards leaves ring room.
      className="group ring-offset-background focus-visible:ring-ring flex flex-col gap-1.5 rounded-md focus-visible:ring-2 focus-visible:ring-offset-2 focus-visible:outline-none"
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
