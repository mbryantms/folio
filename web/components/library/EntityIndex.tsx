"use client";

import { Search, X } from "lucide-react";
import Link from "next/link";
import * as React from "react";

import { PageHeader } from "@/components/admin/PageHeader";
import { AtoZJumpRail } from "@/components/library/AtoZJumpRail";
import { Skeleton } from "@/components/ui/skeleton";
import { useEntityListInfinite, type EntityKindPath } from "@/lib/api/queries";
import type { EntityListItem } from "@/lib/api/types";
import { ENTITY_KINDS, entityUrl } from "@/lib/entities";
import { useInfiniteSentinel } from "@/lib/ui/use-infinite-sentinel";

const SEARCH_DEBOUNCE_MS = 250;

/** Browse index for the WP-5.5 entity kinds (`/characters`, `/teams`,
 *  `/arcs`, `/publishers`). Same shape as the creators index — search
 *  box, A–Z jump rail, compact name cards — backed by the cursor-
 *  paginated `GET /<kind>` endpoint so the directory never silently
 *  truncates. The name search and the letter bucket are server-side
 *  params, not client filters. */
export function EntityIndex({
  kind,
  initialStartsWith,
}: {
  kind: EntityKindPath;
  initialStartsWith?: string | null;
}) {
  const meta = ENTITY_KINDS[kind];
  const [startsWith, setStartsWith] = React.useState<string | null>(
    initialStartsWith ?? null,
  );
  const [rawQ, setRawQ] = React.useState("");
  const [q, setQ] = React.useState("");
  React.useEffect(() => {
    const t = setTimeout(() => setQ(rawQ.trim()), SEARCH_DEBOUNCE_MS);
    return () => clearTimeout(t);
  }, [rawQ]);

  React.useEffect(() => {
    if (typeof window === "undefined") return;
    const url = new URL(window.location.href);
    if (startsWith) url.searchParams.set("starts_with", startsWith);
    else url.searchParams.delete("starts_with");
    window.history.replaceState({}, "", url.toString());
  }, [startsWith]);

  const query = useEntityListInfinite(kind, {
    limit: 60,
    starts_with: startsWith ?? undefined,
    q: q || undefined,
  });
  const sentinelRef = useInfiniteSentinel(query);
  const items = React.useMemo(
    () => query.data?.pages.flatMap((p) => p.items) ?? [],
    [query.data],
  );
  const total = query.data?.pages[0]?.total ?? undefined;

  return (
    <div className="space-y-6">
      <PageHeader
        title={meta.plural}
        description={
          total != null
            ? `${total.toLocaleString()} ${total === 1 ? meta.noun : `${meta.noun}s`} across your libraries`
            : `Every ${meta.noun} in your libraries`
        }
      />

      <div className="border-border bg-card focus-within:ring-ring flex items-center gap-2 rounded-md border px-3 py-2 shadow-sm focus-within:ring-2">
        <Search
          aria-hidden="true"
          className="text-muted-foreground size-4 shrink-0"
        />
        <input
          type="search"
          value={rawQ}
          onChange={(e) => setRawQ(e.target.value)}
          placeholder={`Search ${meta.plural.toLowerCase()} by name…`}
          aria-label={`Search ${meta.plural.toLowerCase()}`}
          className="placeholder:text-muted-foreground w-full bg-transparent text-sm focus:outline-none"
        />
        {rawQ ? (
          <button
            type="button"
            onClick={() => setRawQ("")}
            aria-label="Clear search"
            className="text-muted-foreground hover:text-foreground shrink-0"
          >
            <X className="size-4" />
          </button>
        ) : null}
      </div>

      <AtoZJumpRail value={startsWith} onSelect={setStartsWith} />

      {query.isLoading ? (
        <EntityGridSkeleton />
      ) : query.isError ? (
        <p className="text-muted-foreground text-sm">
          Couldn&apos;t load {meta.plural.toLowerCase()}. Try refreshing.
        </p>
      ) : items.length === 0 ? (
        <p className="text-muted-foreground text-sm">
          No {meta.plural.toLowerCase()} match.
        </p>
      ) : (
        <ul
          role="list"
          className="grid gap-3"
          style={{
            gridTemplateColumns: "repeat(auto-fill, minmax(220px, 1fr))",
          }}
        >
          {items.map((item) => (
            <li key={item.id}>
              <EntityCard kind={kind} item={item} />
            </li>
          ))}
        </ul>
      )}

      <div
        ref={sentinelRef}
        aria-hidden="true"
        className={query.hasNextPage ? "h-12" : "hidden"}
      />
      {query.isFetchingNextPage ? (
        <p className="text-muted-foreground text-center text-xs">
          Loading more…
        </p>
      ) : null}
    </div>
  );
}

function EntityCard({
  kind,
  item,
}: {
  kind: EntityKindPath;
  item: EntityListItem;
}) {
  const hasIssues = ENTITY_KINDS[kind].hasIssues;
  return (
    <Link
      href={entityUrl(kind, item.slug)}
      className="border-border bg-card hover:bg-muted/40 flex h-full flex-col gap-1 rounded-lg border p-3 transition-colors"
    >
      <span className="text-foreground leading-snug font-medium">
        {item.name}
      </span>
      <span className="text-muted-foreground text-xs tabular-nums">
        {item.series_count} series
        {hasIssues
          ? ` · ${item.issue_count} ${item.issue_count === 1 ? "issue" : "issues"}`
          : null}
      </span>
    </Link>
  );
}

export function EntityGridSkeleton() {
  return (
    <ul
      role="list"
      className="grid gap-3"
      style={{ gridTemplateColumns: "repeat(auto-fill, minmax(220px, 1fr))" }}
    >
      {Array.from({ length: 12 }).map((_, i) => (
        <li key={i}>
          <div className="border-border bg-card flex h-full flex-col gap-2 rounded-lg border p-3">
            <Skeleton className="h-4 w-3/4" />
            <Skeleton className="h-3 w-1/2" />
          </div>
        </li>
      ))}
    </ul>
  );
}
