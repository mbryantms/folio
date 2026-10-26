"use client";

import * as React from "react";

import { PageHeader } from "@/components/admin/PageHeader";
import { EntityInsights } from "@/components/library/EntityInsights";
import {
  EntityProfile,
  EntitySections,
} from "@/components/library/EntityProfile";
import { IssueCard, IssueCardSkeleton } from "@/components/library/IssueCard";
import { SeriesCard } from "@/components/library/SeriesCard";
import {
  Tabs,
  TabsContent,
  TabsList,
  TabsTrigger,
} from "@/components/folio/tabs";
import { useRefreshEntityProfile } from "@/lib/api/mutations";
import {
  useArcTieInsInfinite,
  useEntityDetail,
  useEntityIssuesInfinite,
  useEntitySeriesInfinite,
  useMe,
  type EntityKindPath,
} from "@/lib/api/queries";
import type { ArcTieInView, EntityDetailView } from "@/lib/api/types";
import { scopeCaption } from "@/lib/relationships";
import { ENTITY_KINDS } from "@/lib/entities";
import { useInfiniteSentinel } from "@/lib/ui/use-infinite-sentinel";

const GRID_STYLE: React.CSSProperties = {
  gridTemplateColumns: "repeat(auto-fill, minmax(160px, 1fr))",
};

/** Shared landing page for characters / teams / story arcs / publishers
 *  (WP-5.5), built on the creators-page template: a `PageHeader` with the
 *  entity's counts + provider extras, then cursor-paginated grids. Issue
 *  kinds get an issues tab (arc = reading order) next to the series tab;
 *  publishers are series-only. Both grids are `useInfiniteQuery` walks
 *  with an IntersectionObserver sentinel, so nothing truncates. */
export function EntityDetail({
  kind,
  detail: initial,
}: {
  kind: EntityKindPath;
  detail: EntityDetailView;
}) {
  const meta = ENTITY_KINDS[kind];
  // Live copy of the header payload: polled while the provider profile
  // is being fetched so the wiki block fills in without a reload.
  const live = useEntityDetail(kind, initial.slug, initial);
  const detail = live.data ?? initial;
  const me = useMe();
  const isAdmin = me.data?.role === "admin";
  const refresh = useRefreshEntityProfile(kind, detail.slug);
  const parts: string[] = [];
  if (meta.hasIssues) {
    parts.push(
      `${detail.issue_count} ${detail.issue_count === 1 ? "issue" : "issues"}`,
    );
  }
  parts.push(`${detail.series_count} series`);
  if (detail.real_name) parts.push(`Real name: ${detail.real_name}`);
  if (detail.founded_year) parts.push(`Founded ${detail.founded_year}`);
  const firstAppearance = detail.profile?.facts.first_appearance?.label;
  if (firstAppearance) parts.push(`First appeared in ${firstAppearance}`);
  const sections = detail.profile?.sections ?? [];

  return (
    <div className="space-y-6">
      <PageHeader
        title={detail.name}
        description={parts.join(" · ")}
        breadcrumbs={[{ label: meta.plural, href: `/${kind}` }]}
      />
      {kind === "publishers" ? (
        <PublisherBlurb detail={detail} />
      ) : (
        <>
          <EntityProfile
            kind={kind}
            detail={detail}
            isAdmin={isAdmin}
            onRefresh={() => refresh.mutate()}
            refreshing={refresh.isPending}
          />
          {sections.length > 0 ? <EntitySections sections={sections} /> : null}
          <EntityInsights kind={kind} slug={detail.slug} />
        </>
      )}

      {meta.hasIssues ? (
        <EntityTabs
          kind={kind}
          slug={detail.slug}
          issuesLabel={meta.issuesLabel}
        />
      ) : (
        <SeriesGrid kind={kind} slug={detail.slug} />
      )}
    </div>
  );
}

/** Publishers have no provider profile: the short fields only. */
function PublisherBlurb({ detail }: { detail: EntityDetailView }) {
  if (!detail.description && !detail.aliases.length) return null;
  return (
    <section className="min-w-0 space-y-3">
      {detail.description ? (
        <p className="text-muted-foreground max-w-prose text-sm whitespace-pre-line">
          {detail.description}
        </p>
      ) : null}
      {detail.aliases.length ? (
        <p className="text-muted-foreground text-xs">
          Also known as {detail.aliases.join(", ")}
        </p>
      ) : null}
    </section>
  );
}

/** Issue-kind tabs. Story arcs add a **Tie-ins** tab (WP-7.7) when any
 *  series ties in to the arc (`GET /arcs/{slug}/tie-ins`, first page
 *  fetched up front for the count). */
function EntityTabs({
  kind,
  slug,
  issuesLabel,
}: {
  kind: EntityKindPath;
  slug: string;
  issuesLabel: string;
}) {
  const isArc = kind === "arcs";
  const tieIns = useArcTieInsInfinite(isArc ? slug : "");
  const tieInTotal = isArc ? (tieIns.data?.pages[0]?.total ?? 0) : 0;
  return (
    <Tabs defaultValue="issues">
      <TabsList>
        <TabsTrigger value="issues">{issuesLabel}</TabsTrigger>
        <TabsTrigger value="series">Series</TabsTrigger>
        {tieInTotal > 0 && (
          <TabsTrigger value="tie-ins">
            Tie-ins
            <span className="text-muted-foreground ml-1.5 text-xs tabular-nums">
              {tieInTotal}
            </span>
          </TabsTrigger>
        )}
      </TabsList>
      <TabsContent value="issues" className="pt-4">
        <IssuesGrid kind={kind} slug={slug} />
      </TabsContent>
      <TabsContent value="series" className="pt-4">
        <SeriesGrid kind={kind} slug={slug} />
      </TabsContent>
      {tieInTotal > 0 && (
        <TabsContent value="tie-ins" className="pt-4">
          <ArcTieIns slug={slug} />
        </TabsContent>
      )}
    </Tabs>
  );
}

/** Group a role-ordered tie-in list into contiguous role sections. The
 *  server orders by role (prelude, main story, tie-in, aftermath), so a
 *  later page only ever extends the last group or appends new ones. */
export function groupTieIns(
  items: ArcTieInView[],
): Array<{ label: string; items: ArcTieInView[] }> {
  const out: Array<{ label: string; items: ArcTieInView[] }> = [];
  for (const item of items) {
    const label = tieInGroupLabel(item);
    const last = out[out.length - 1];
    if (last && last.label === label) last.items.push(item);
    else out.push({ label, items: [item] });
  }
  return out;
}

function tieInGroupLabel(item: ArcTieInView): string {
  switch (item.qualifier) {
    case "prelude":
      return "Preludes";
    case "main":
      return "Main story";
    case "aftermath":
      return "Aftermath";
    default:
      return "Tie-ins";
  }
}

function ArcTieIns({ slug }: { slug: string }) {
  const query = useArcTieInsInfinite(slug);
  const sentinelRef = useInfiniteSentinel(query);
  const items = React.useMemo(
    () => query.data?.pages.flatMap((p) => p.items) ?? [],
    [query.data],
  );
  if (query.isLoading) return <SkeletonGrid />;
  if (query.isError) {
    return (
      <p className="text-muted-foreground text-sm">
        Couldn&apos;t load tie-ins. Try refreshing.
      </p>
    );
  }
  return (
    <div className="space-y-8">
      {groupTieIns(items).map((g) => (
        <section key={g.label} className="space-y-3">
          <h2 className="text-foreground text-sm font-medium">{g.label}</h2>
          <ul role="list" className="grid gap-4" style={GRID_STYLE}>
            {g.items.map((t) => {
              const caption = scopeCaption(t);
              return (
                <li key={t.id} className="flex flex-col">
                  <SeriesCard series={t.series} size="md" />
                  {caption ? (
                    <p
                      className="text-muted-foreground line-clamp-2 px-2 text-xs"
                      title={caption}
                    >
                      {caption}
                    </p>
                  ) : null}
                </li>
              );
            })}
          </ul>
        </section>
      ))}
      <Sentinel
        ref={sentinelRef}
        active={query.hasNextPage}
        loading={query.isFetchingNextPage}
      />
    </div>
  );
}

function IssuesGrid({ kind, slug }: { kind: EntityKindPath; slug: string }) {
  const query = useEntityIssuesInfinite(kind, slug);
  const sentinelRef = useInfiniteSentinel(query);
  const items = React.useMemo(
    () => query.data?.pages.flatMap((p) => p.items) ?? [],
    [query.data],
  );
  if (query.isLoading) return <SkeletonGrid />;
  if (query.isError) {
    return (
      <p className="text-muted-foreground text-sm">
        Couldn&apos;t load issues. Try refreshing.
      </p>
    );
  }
  if (items.length === 0) {
    return <p className="text-muted-foreground text-sm">No issues.</p>;
  }
  return (
    <>
      <ul role="list" className="grid gap-4" style={GRID_STYLE}>
        {items.map((issue) => (
          <li key={issue.id}>
            <IssueCard issue={issue} />
          </li>
        ))}
      </ul>
      <Sentinel
        ref={sentinelRef}
        active={query.hasNextPage}
        loading={query.isFetchingNextPage}
      />
    </>
  );
}

function SeriesGrid({ kind, slug }: { kind: EntityKindPath; slug: string }) {
  const query = useEntitySeriesInfinite(kind, slug);
  const sentinelRef = useInfiniteSentinel(query);
  const items = React.useMemo(
    () => query.data?.pages.flatMap((p) => p.items) ?? [],
    [query.data],
  );
  if (query.isLoading) return <SkeletonGrid />;
  if (query.isError) {
    return (
      <p className="text-muted-foreground text-sm">
        Couldn&apos;t load series. Try refreshing.
      </p>
    );
  }
  if (items.length === 0) {
    return <p className="text-muted-foreground text-sm">No series.</p>;
  }
  return (
    <>
      <ul role="list" className="grid gap-4" style={GRID_STYLE}>
        {items.map((s) => (
          <li key={s.id}>
            <SeriesCard series={s} size="md" />
          </li>
        ))}
      </ul>
      <Sentinel
        ref={sentinelRef}
        active={query.hasNextPage}
        loading={query.isFetchingNextPage}
      />
    </>
  );
}

function Sentinel({
  ref,
  active,
  loading,
}: {
  ref: React.Ref<HTMLDivElement>;
  active: boolean;
  loading: boolean;
}) {
  return (
    <>
      <div
        ref={ref}
        aria-hidden="true"
        className={active ? "h-12" : "hidden"}
      />
      {loading ? (
        <p className="text-muted-foreground text-center text-xs">
          Loading more…
        </p>
      ) : null}
    </>
  );
}

function SkeletonGrid() {
  return (
    <ul role="list" className="grid gap-4" style={GRID_STYLE}>
      {Array.from({ length: 12 }).map((_, i) => (
        <li key={i}>
          <IssueCardSkeleton />
        </li>
      ))}
    </ul>
  );
}
