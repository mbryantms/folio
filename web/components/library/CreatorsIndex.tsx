"use client";

import { Search, X } from "lucide-react";
import Link from "next/link";
import * as React from "react";

import { AtoZJumpRail } from "@/components/library/AtoZJumpRail";
import { PageHeader } from "@/components/admin/PageHeader";
import { Badge } from "@/components/ui/badge";
import { Skeleton } from "@/components/ui/skeleton";
import { useCreatorsInfinite, usePeopleSearch } from "@/lib/api/queries";
import type { CreatorListItem } from "@/lib/api/types";

const SEARCH_DEBOUNCE_MS = 250;
/** Min query length before switching from browse to search — matches the
 *  other global-search categories (and the `/people` server gate). */
const SEARCH_MIN = 2;

/** Roles shown on a creator card before collapsing to "+N". Keeps the
 *  card to a single chip row on the typical grid width. */
const ROLE_CHIP_CAP = 3;

/** Canonical role order — mirrors the backend `ROLE_ORDER` so the chip
 *  row on a card reads the same as the detail page header. */
const CANONICAL_ROLES: readonly string[] = [
  "writer",
  "penciller",
  "inker",
  "colorist",
  "letterer",
  "cover_artist",
  "editor",
  "translator",
];

/** Browse index for `GET /creators` (audit A11). Cursor-paginated so
 *  the directory never silently truncates — an IntersectionObserver
 *  sentinel walks every page as the user scrolls. Each card links to
 *  the creator's detail page (`/creators/<slug>`), falling back to the
 *  legacy library-grid `?credits=<name>` filter for names the `person`
 *  backfill hasn't slugged yet. */
export function CreatorsIndex({
  initialStartsWith,
}: {
  /** Server-parsed `?starts_with=` jump-rail bucket for deep-links. */
  initialStartsWith?: string | null;
}) {
  const [startsWith, setStartsWith] = React.useState<string | null>(
    initialStartsWith ?? null,
  );

  // Search box: empty → alphabetical browse (the infinite directory + A–Z
  // rail); ≥2 chars → `/people` fuzzy search (trigram, ranked). The browse
  // endpoint can't search (empty `q` returns nothing) so the two are
  // distinct hooks; we just render one or the other.
  const [rawQ, setRawQ] = React.useState("");
  const [debouncedQ, setDebouncedQ] = React.useState("");
  React.useEffect(() => {
    const t = setTimeout(() => setDebouncedQ(rawQ.trim()), SEARCH_DEBOUNCE_MS);
    return () => clearTimeout(t);
  }, [rawQ]);
  const searching = debouncedQ.length >= SEARCH_MIN;

  const query = useCreatorsInfinite(
    { limit: 60, starts_with: startsWith ?? undefined },
    { enabled: !searching },
  );
  const search = usePeopleSearch(searching ? { q: debouncedQ, limit: 60 } : {});
  const { hasNextPage, isFetchingNextPage, fetchNextPage } = query;

  // Keep `?starts_with=` on the URL so a chosen letter is shareable and
  // survives reload (the server page re-seeds `initialStartsWith`).
  React.useEffect(() => {
    if (typeof window === "undefined") return;
    const url = new URL(window.location.href);
    if (startsWith) url.searchParams.set("starts_with", startsWith);
    else url.searchParams.delete("starts_with");
    window.history.replaceState({}, "", url.toString());
  }, [startsWith]);

  // Same sentinel shape as `IssuesPanel` / `MarkersList` — depend on the
  // three fields (not the whole result object) so the observer isn't torn
  // down on every render.
  const sentinelRef = React.useRef<HTMLDivElement | null>(null);
  React.useEffect(() => {
    const el = sentinelRef.current;
    if (!el) return;
    const obs = new IntersectionObserver(
      (entries) => {
        if (entries.some((e) => e.isIntersecting)) {
          if (hasNextPage && !isFetchingNextPage) {
            void fetchNextPage();
          }
        }
      },
      { rootMargin: "400px" },
    );
    obs.observe(el);
    return () => obs.disconnect();
  }, [hasNextPage, isFetchingNextPage, fetchNextPage]);

  const items = React.useMemo(
    () => query.data?.pages.flatMap((p) => p.items) ?? [],
    [query.data],
  );
  const total = query.data?.pages[0]?.total ?? undefined;
  const searchItems = search.data?.items ?? [];

  return (
    <div className="space-y-6">
      <PageHeader
        title="Creators"
        description={
          searching
            ? `Searching for “${debouncedQ}”`
            : total != null
              ? `${total.toLocaleString()} ${total === 1 ? "creator" : "creators"} across your libraries`
              : "Everyone credited across your libraries"
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
          placeholder="Search creators by name…"
          aria-label="Search creators"
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

      {searching ? (
        /* ── Search mode: ranked /people results, no A–Z rail. ── */
        search.isLoading ? (
          <CreatorGridSkeleton />
        ) : search.isError ? (
          <p className="text-muted-foreground text-sm">
            Couldn&apos;t search creators. Try again.
          </p>
        ) : searchItems.length === 0 ? (
          <p className="text-muted-foreground text-sm">
            No creators match “{debouncedQ}”.
          </p>
        ) : (
          <CreatorGrid items={searchItems} />
        )
      ) : (
        /* ── Browse mode: A–Z rail + the infinite directory. ── */
        <>
          <AtoZJumpRail value={startsWith} onSelect={setStartsWith} />

          {query.isLoading ? (
            <CreatorGridSkeleton />
          ) : query.isError ? (
            <p className="text-muted-foreground text-sm">
              Couldn&apos;t load creators. Try refreshing.
            </p>
          ) : items.length === 0 ? (
            <p className="text-muted-foreground text-sm">
              {startsWith
                ? `No creators start with "${startsWith === "#" ? "#" : startsWith.toUpperCase()}".`
                : "No creators are credited in any library you can access yet."}
            </p>
          ) : (
            <CreatorGrid items={items} />
          )}

          <div
            ref={sentinelRef}
            aria-hidden="true"
            className={hasNextPage ? "h-12" : "hidden"}
          />
          {isFetchingNextPage ? (
            <p className="text-muted-foreground text-center text-xs">
              Loading more…
            </p>
          ) : null}
        </>
      )}
    </div>
  );
}

function CreatorGrid({ items }: { items: CreatorListItem[] }) {
  return (
    <ul
      role="list"
      className="grid gap-3"
      style={{ gridTemplateColumns: "repeat(auto-fill, minmax(220px, 1fr))" }}
    >
      {items.map((c) => (
        <li key={c.slug ?? c.person}>
          <CreatorCard creator={c} />
        </li>
      ))}
    </ul>
  );
}

function CreatorCard({ creator }: { creator: CreatorListItem }) {
  const href = creator.slug
    ? `/creators/${encodeURIComponent(creator.slug)}`
    : `/?library=all&credits=${encodeURIComponent(creator.person)}`;
  const roles = sortRoles(creator.roles);
  const shown = roles.slice(0, ROLE_CHIP_CAP);
  const overflow = roles.length - shown.length;
  return (
    <Link
      href={href}
      title={`Open ${creator.person}'s creator page`}
      className="border-border bg-card hover:bg-muted/40 flex h-full flex-col gap-2 rounded-lg border p-3 transition-colors"
    >
      <div className="flex items-start justify-between gap-2">
        <span className="text-foreground leading-snug font-medium">
          {creator.person}
        </span>
        <span className="text-muted-foreground shrink-0 text-xs tabular-nums">
          {creator.credit_count}
        </span>
      </div>
      <div className="flex flex-wrap gap-1">
        {shown.map((role) => (
          <Badge key={role} variant="secondary" className="text-[11px]">
            {formatRole(role)}
          </Badge>
        ))}
        {overflow > 0 ? (
          <Badge variant="outline" className="text-[11px]">
            +{overflow}
          </Badge>
        ) : null}
      </div>
    </Link>
  );
}

function CreatorGridSkeleton() {
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
            <div className="flex gap-1">
              <Skeleton className="h-4 w-12" />
              <Skeleton className="h-4 w-10" />
            </div>
          </div>
        </li>
      ))}
    </ul>
  );
}

/** Title-case a role token ("cover_artist" → "Cover artist"). */
function formatRole(role: string): string {
  return role
    .split("_")
    .map((s, i) =>
      s.length === 0
        ? s
        : i === 0
          ? s[0]!.toUpperCase() + s.slice(1)
          : s.toLowerCase(),
    )
    .join(" ");
}

/** Sort roles by canonical order, unknown roles alphabetical at the end. */
function sortRoles(roles: readonly string[]): string[] {
  return [...roles].sort((a, b) => {
    const ai = CANONICAL_ROLES.indexOf(a);
    const bi = CANONICAL_ROLES.indexOf(b);
    if (ai === -1 && bi === -1) return a.localeCompare(b);
    if (ai === -1) return 1;
    if (bi === -1) return -1;
    return ai - bi;
  });
}
