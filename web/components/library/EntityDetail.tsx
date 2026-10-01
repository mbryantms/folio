"use client";

import * as React from "react";

import { PageHeader } from "@/components/admin/PageHeader";
import { IssueCard, IssueCardSkeleton } from "@/components/library/IssueCard";
import { ProviderCoverImage } from "@/components/library/ProviderCoverImage";
import { SeriesCard } from "@/components/library/SeriesCard";
import { Badge } from "@/components/ui/badge";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import {
  useEntityIssuesInfinite,
  useEntitySeriesInfinite,
  type EntityKindPath,
} from "@/lib/api/queries";
import type { EntityDetailView } from "@/lib/api/types";
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
  detail,
}: {
  kind: EntityKindPath;
  detail: EntityDetailView;
}) {
  const meta = ENTITY_KINDS[kind];
  const parts: string[] = [];
  if (meta.hasIssues) {
    parts.push(
      `${detail.issue_count} ${detail.issue_count === 1 ? "issue" : "issues"}`,
    );
  }
  parts.push(`${detail.series_count} series`);
  if (detail.real_name) parts.push(`Real name: ${detail.real_name}`);
  if (detail.founded_year) parts.push(`Founded ${detail.founded_year}`);

  return (
    <div className="space-y-6">
      <PageHeader
        title={detail.name}
        description={parts.join(" · ")}
        breadcrumbs={[{ label: meta.plural, href: `/${kind}` }]}
      />
      {detail.image_url || detail.description || detail.aliases.length ? (
        <section className="flex flex-col gap-4 sm:flex-row">
          {detail.image_url ? (
            // Provider-hosted image; not routed through next/image so the
            // CSP img-src policy (not a loader allowlist) governs it. A
            // CDN that refuses the hotlink collapses to nothing.
            <ProviderCoverImage
              src={detail.image_url}
              alt=""
              className="bg-muted h-40 w-auto self-start rounded-md object-cover"
              placeholderClassName="hidden"
            />
          ) : null}
          <div className="min-w-0 space-y-3">
            {detail.description ? (
              <p className="text-muted-foreground max-w-prose text-sm whitespace-pre-line">
                {detail.description}
              </p>
            ) : null}
            {detail.aliases.length ? (
              <div className="flex flex-wrap items-center gap-1.5">
                <span className="text-muted-foreground text-xs font-semibold tracking-wider uppercase">
                  Also known as
                </span>
                {detail.aliases.map((a) => (
                  <Badge key={a} variant="secondary" className="font-normal">
                    {a}
                  </Badge>
                ))}
              </div>
            ) : null}
          </div>
        </section>
      ) : null}

      {meta.hasIssues ? (
        <Tabs defaultValue="issues">
          <TabsList>
            <TabsTrigger value="issues">{meta.issuesLabel}</TabsTrigger>
            <TabsTrigger value="series">Series</TabsTrigger>
          </TabsList>
          <TabsContent value="issues" className="pt-4">
            <IssuesGrid kind={kind} slug={detail.slug} />
          </TabsContent>
          <TabsContent value="series" className="pt-4">
            <SeriesGrid kind={kind} slug={detail.slug} />
          </TabsContent>
        </Tabs>
      ) : (
        <SeriesGrid kind={kind} slug={detail.slug} />
      )}
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
