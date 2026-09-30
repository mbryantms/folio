"use client";

import * as React from "react";
import Link from "next/link";
import { Check, Loader2, Pencil, Trash2, Undo2 } from "lucide-react";

import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from "@/components/ui/alert-dialog";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { FilterPill } from "@/components/ui/filter-pill";
import { Skeleton } from "@/components/ui/skeleton";
import {
  useClearDuplicateDecision,
  useSetDuplicateDecision,
} from "@/lib/api/mutations";
import { useDuplicatesInfinite } from "@/lib/api/queries";
import type {
  DuplicateGroupView,
  DuplicateIssueView,
  DuplicateKindFilter,
} from "@/lib/api/types";

const FILTERS: { value: DuplicateKindFilter; label: string }[] = [
  { value: "all", label: "All" },
  { value: "hash", label: "Identical files" },
  { value: "number", label: "Same number" },
  { value: "cover", label: "Similar covers" },
];

const KIND_LABEL: Record<DuplicateGroupView["kind"], string> = {
  hash: "Identical file",
  number: "Same number",
  cover: "Similar cover",
};

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  const units = ["KB", "MB", "GB"];
  let v = n / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i += 1;
  }
  return `${v.toFixed(v >= 10 ? 0 : 1)} ${units[i]}`;
}

function issueLabel(issue: DuplicateIssueView): string {
  const num = issue.number_raw ? `#${issue.number_raw}` : null;
  const special = issue.special_type ? ` (${issue.special_type})` : "";
  return [num, issue.title].filter(Boolean).join(" · ") + special || issue.slug;
}

/**
 * Duplicates page for one library (WP-3.3). Groups come from
 * `GET /libraries/{slug}/duplicates` — cursor-paginated, the kind filter
 * is a server param, and the next page loads when the sentinel scrolls
 * into view (IssuesPanel template). Each copy offers keep / soft-remove
 * (AlertDialog confirm) / open editor.
 */
export function DuplicatesPanel({ libraryId }: { libraryId: string }) {
  const [kind, setKind] = React.useState<DuplicateKindFilter>("all");
  const query = useDuplicatesInfinite(libraryId, kind);
  const { data, isLoading, error, hasNextPage, isFetchingNextPage } = query;
  const fetchNextPage = query.fetchNextPage;
  const sentinelRef = React.useRef<HTMLDivElement | null>(null);

  // Auto-fetch the next page when the sentinel scrolls into view. Depend
  // on the fields, not the result object (fresh identity per render).
  React.useEffect(() => {
    const el = sentinelRef.current;
    if (!el) return;
    const obs = new IntersectionObserver(
      (entries) => {
        if (entries.some((e) => e.isIntersecting)) {
          if (hasNextPage && !isFetchingNextPage) void fetchNextPage();
        }
      },
      { rootMargin: "400px 0px" },
    );
    obs.observe(el);
    return () => obs.disconnect();
  }, [hasNextPage, isFetchingNextPage, fetchNextPage]);

  const groups = data?.pages.flatMap((p) => p.items) ?? [];
  const first = data?.pages[0];
  const counts = first?.counts;
  const total = first?.total ?? groups.length;

  return (
    <div className="space-y-4">
      <p className="text-muted-foreground max-w-3xl text-sm">
        Copies of the same issue inside this library: identical files, issues
        sharing a series + number, and near-identical covers within a series.
        The same file in another library is never listed. Keep the copies you
        want; soft-removed copies stay on disk and can be restored from the
        Removed tab.
      </p>
      <div className="flex flex-wrap items-center gap-2" role="group">
        {FILTERS.map((f) => (
          <FilterPill
            key={f.value}
            active={kind === f.value}
            onClick={() => setKind(f.value)}
            count={
              f.value === "all" || !counts
                ? undefined
                : counts[f.value as keyof typeof counts]
            }
          >
            {f.label}
          </FilterPill>
        ))}
      </div>

      {isLoading ? (
        <Skeleton className="h-64 w-full" />
      ) : error ? (
        <p className="text-destructive text-sm">{error.message}</p>
      ) : groups.length === 0 ? (
        <p className="border-border bg-card/40 text-muted-foreground rounded-md border border-dashed px-4 py-12 text-center text-sm">
          No duplicates found.
        </p>
      ) : (
        <>
          <p className="text-muted-foreground text-xs">
            {total.toLocaleString()} group{total === 1 ? "" : "s"}
          </p>
          <ul className="space-y-3">
            {groups.map((g) => (
              <DuplicateGroupCard key={g.key} group={g} libraryId={libraryId} />
            ))}
          </ul>
        </>
      )}

      <div
        ref={sentinelRef}
        aria-hidden
        className={hasNextPage ? "h-12" : "hidden"}
      />
      {isFetchingNextPage ? (
        <div className="text-muted-foreground flex items-center justify-center gap-2 text-xs">
          <Loader2 className="size-3.5 animate-spin" /> Loading more…
        </div>
      ) : null}
    </div>
  );
}

function DuplicateGroupCard({
  group,
  libraryId,
}: {
  group: DuplicateGroupView;
  libraryId: string;
}) {
  return (
    <li className="border-border bg-card rounded-md border">
      <header className="border-border flex flex-wrap items-center gap-2 border-b px-4 py-2">
        <Badge variant="secondary">{KIND_LABEL[group.kind]}</Badge>
        <Link
          href={`/series/${group.series_slug}`}
          className="text-foreground text-sm font-medium hover:underline"
        >
          {group.series_name}
        </Link>
        {group.max_cover_distance != null ? (
          <span className="text-muted-foreground text-xs">
            cover distance ≤ {group.max_cover_distance}
          </span>
        ) : null}
        <span className="text-muted-foreground ml-auto text-xs">
          {group.issues.length} copies
        </span>
      </header>
      <ul className="divide-border divide-y">
        {group.issues.map((issue) => (
          <DuplicateIssueRow
            key={issue.id}
            issue={issue}
            libraryId={libraryId}
          />
        ))}
      </ul>
    </li>
  );
}

function DuplicateIssueRow({
  issue,
  libraryId,
}: {
  issue: DuplicateIssueView;
  libraryId: string;
}) {
  const setDecision = useSetDuplicateDecision(libraryId);
  const clearDecision = useClearDuplicateDecision(libraryId);
  const ref = { seriesSlug: issue.series_slug, issueSlug: issue.slug };
  const busy = setDecision.isPending || clearDecision.isPending;
  const issueHref = `/series/${issue.series_slug}/issues/${issue.slug}`;

  return (
    <li className="flex flex-wrap items-center gap-3 px-4 py-3 sm:flex-nowrap">
      <div className="bg-muted relative h-16 w-11 shrink-0 overflow-hidden rounded-sm">
        {issue.cover_url ? (
          // eslint-disable-next-line @next/next/no-img-element
          <img
            src={issue.cover_url}
            alt=""
            loading="lazy"
            className="absolute inset-0 h-full w-full object-cover"
          />
        ) : null}
      </div>
      <div className="min-w-0 flex-1 space-y-0.5">
        <div className="flex flex-wrap items-center gap-2">
          <Link
            href={issueHref}
            className="text-foreground text-sm font-medium hover:underline"
          >
            {issueLabel(issue)}
          </Link>
          {issue.series_name ? (
            <span className="text-muted-foreground text-xs">
              {issue.series_name}
            </span>
          ) : null}
          {issue.decision === "keep" ? (
            <Badge variant="outline">Kept</Badge>
          ) : null}
        </div>
        <p className="text-muted-foreground font-mono text-xs wrap-anywhere">
          {issue.file_path}
        </p>
        <p className="text-muted-foreground text-xs">
          {formatBytes(issue.file_size)}
          {issue.page_count != null ? ` · ${issue.page_count} pages` : ""}
          {issue.state !== "active" ? ` · ${issue.state}` : ""}
        </p>
      </div>
      <div className="flex shrink-0 items-center gap-1">
        {issue.decision === "keep" ? (
          <Button
            size="sm"
            variant="ghost"
            disabled={busy}
            onClick={() => clearDecision.mutate(ref)}
          >
            <Undo2 className="mr-1 size-3.5" />
            Undo keep
          </Button>
        ) : (
          <Button
            size="sm"
            variant="ghost"
            disabled={busy}
            onClick={() => setDecision.mutate({ ...ref, decision: "keep" })}
          >
            <Check className="mr-1 size-3.5" />
            Keep
          </Button>
        )}
        <Button size="sm" variant="ghost" asChild>
          <Link href={`${issueHref}?edit=1`}>
            <Pencil className="mr-1 size-3.5" />
            Edit
          </Link>
        </Button>
        <AlertDialog>
          <AlertDialogTrigger asChild>
            <Button size="sm" variant="outline" disabled={busy}>
              <Trash2 className="mr-1 size-3.5" />
              Remove
            </Button>
          </AlertDialogTrigger>
          <AlertDialogContent>
            <AlertDialogHeader>
              <AlertDialogTitle>Soft-remove this copy?</AlertDialogTitle>
              <AlertDialogDescription>
                <span className="font-mono wrap-anywhere">
                  {issue.file_path}
                </span>{" "}
                is hidden from the library and stays hidden across rescans. The
                file on disk is not touched. Restore it any time from the
                Removed tab.
              </AlertDialogDescription>
            </AlertDialogHeader>
            <AlertDialogFooter>
              <AlertDialogCancel>Cancel</AlertDialogCancel>
              <AlertDialogAction
                onClick={() =>
                  setDecision.mutate({ ...ref, decision: "remove" })
                }
              >
                Remove
              </AlertDialogAction>
            </AlertDialogFooter>
          </AlertDialogContent>
        </AlertDialog>
      </div>
    </li>
  );
}
