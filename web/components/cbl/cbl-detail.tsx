"use client";

import * as React from "react";
import { useRouter } from "next/navigation";
import { useVirtualizer } from "@tanstack/react-virtual";
import {
  AlertTriangle,
  Check,
  CheckCircle2,
  ExternalLink,
  Github,
  HelpCircle,
  Loader2,
  Pencil,
  RefreshCw,
  Trash2,
  Upload,
  X,
} from "lucide-react";

import { EditCblMetadataDialog } from "@/components/saved-views/EditCblMetadataDialog";

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
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { useCblHideMissing } from "@/lib/cbl/use-hide-missing";
import { cn } from "@/lib/utils";
import {
  useCblList,
  useCblListEntriesInfinite,
  useCblRefreshLog,
} from "@/lib/api/queries";
import {
  useDeleteCblList,
  useDeleteSavedView,
  useRefreshCblList,
} from "@/lib/api/mutations";
import type {
  CblDetailView,
  CblEntryHydratedView,
  CblMatchStatus,
  RefreshLogEntryView,
  SavedViewView,
} from "@/lib/api/types";

import { ManualMatchPopover } from "./manual-match-popover";

const STATUS_FILTERS: { value: "all" | CblMatchStatus; label: string }[] = [
  { value: "all", label: "All" },
  { value: "matched", label: "Matched" },
  { value: "ambiguous", label: "Ambiguous" },
  { value: "missing", label: "Missing" },
  { value: "manual", label: "Manual" },
];

export function CblDetail({ savedView }: { savedView: SavedViewView }) {
  const listId = savedView.cbl_list_id;
  if (!listId) {
    return (
      <div className="text-destructive rounded-md border p-4 text-sm">
        Saved view is marked as CBL but has no `cbl_list_id`.
      </div>
    );
  }
  return <CblDetailInner savedView={savedView} listId={listId} />;
}

function CblDetailInner({
  savedView,
  listId,
}: {
  savedView: SavedViewView;
  listId: string;
}) {
  const list = useCblList(listId);

  if (list.isLoading) {
    return (
      <div className="text-muted-foreground flex items-center gap-2 py-12 text-sm">
        <Loader2 className="h-4 w-4 animate-spin" /> Loading list…
      </div>
    );
  }
  if (list.isError || !list.data) {
    return (
      <div className="text-destructive rounded-md border p-4 text-sm">
        Failed to load list: {String(list.error)}
      </div>
    );
  }

  const data = list.data;
  const ambCount = data.stats.ambiguous + data.stats.missing;

  return (
    // Fills the sheet body as a flex column. Tabs picks up the
    // remaining height; the Reading-order tab hands ownership of
    // vertical scroll to its inner virtualizer (so the sheet itself
    // never scrolls). Other tabs scroll their own content.
    <div className="flex min-h-0 flex-1 flex-col gap-4">
      <CblInfoRow list={data} />
      <Tabs
        defaultValue="reading-order"
        className="flex min-h-0 flex-1 flex-col"
      >
        <TabsList>
          <TabsTrigger value="reading-order">Reading order</TabsTrigger>
          <TabsTrigger value="resolution">
            Resolution
            {ambCount > 0 ? (
              <Badge variant="destructive" className="ml-2 px-1.5 py-0 text-xs">
                {ambCount}
              </Badge>
            ) : null}
          </TabsTrigger>
          <TabsTrigger value="history">History</TabsTrigger>
          <TabsTrigger value="settings">Settings</TabsTrigger>
        </TabsList>
        <TabsContent
          value="reading-order"
          className="flex min-h-0 flex-1 flex-col"
        >
          <ReadingOrderTab listId={listId} stats={data.stats} />
        </TabsContent>
        <TabsContent
          value="resolution"
          className="min-h-0 flex-1 overflow-y-auto"
        >
          <ResolutionTab listId={listId} stats={data.stats} />
        </TabsContent>
        <TabsContent value="history" className="min-h-0 flex-1 overflow-y-auto">
          <HistoryTab listId={listId} list={data} />
        </TabsContent>
        <TabsContent
          value="settings"
          className="min-h-0 flex-1 overflow-y-auto"
        >
          <SettingsTab list={data} savedView={savedView} />
        </TabsContent>
      </Tabs>
    </div>
  );
}

/** Info-row shown above the management tabs. Just source badge,
 *  matchers warning, and imported date — the title / Edit / Pin /
 *  Refresh / Export controls live on the wrapping consumption view
 *  (`<CblViewDetail>`) or its parent dialog header. */
export function CblInfoRow({ list }: { list: CblDetailView }) {
  const sourceBadge = (() => {
    if (list.source_kind === "catalog") {
      return (
        <Badge variant="secondary">
          <Github className="mr-1 h-3 w-3" />
          Catalog · {list.catalog_path}
        </Badge>
      );
    }
    if (list.source_kind === "url") {
      return (
        <Badge variant="secondary">
          <ExternalLink className="mr-1 h-3 w-3" /> URL
        </Badge>
      );
    }
    return (
      <Badge variant="secondary">
        <Upload className="mr-1 h-3 w-3" /> Upload
      </Badge>
    );
  })();
  return (
    <div className="flex flex-wrap items-center gap-2 text-sm">
      {sourceBadge}
      {list.parsed_matchers_present ? (
        <Badge variant="outline" className="border-amber-500 text-amber-600">
          <AlertTriangle className="mr-1 h-3 w-3" />
          Matcher rules in source — not evaluated
        </Badge>
      ) : null}
      <span className="text-muted-foreground">
        imported {new Date(list.imported_at).toLocaleDateString()}
      </span>
    </div>
  );
}

function StatsCard({
  stats,
}: {
  stats: {
    total: number;
    matched: number;
    ambiguous: number;
    missing: number;
    manual: number;
  };
}) {
  return (
    <div className="bg-muted/50 grid grid-cols-2 gap-3 rounded-md p-3 text-sm sm:grid-cols-5">
      <Stat label="Total" value={stats.total} />
      <Stat label="Matched" value={stats.matched} tone="ok" />
      <Stat label="Manual" value={stats.manual} tone="ok" />
      <Stat label="Ambiguous" value={stats.ambiguous} tone="warn" />
      <Stat label="Missing" value={stats.missing} tone="bad" />
    </div>
  );
}

function Stat({
  label,
  value,
  tone,
}: {
  label: string;
  value: number;
  tone?: "ok" | "warn" | "bad";
}) {
  const toneClass =
    tone === "ok"
      ? "text-emerald-600 dark:text-emerald-400"
      : tone === "warn"
        ? "text-amber-600 dark:text-amber-400"
        : tone === "bad"
          ? "text-rose-600 dark:text-rose-400"
          : "";
  return (
    <div>
      <div className="text-muted-foreground text-xs tracking-wider uppercase">
        {label}
      </div>
      <div className={`text-xl font-semibold ${toneClass}`}>{value}</div>
    </div>
  );
}

function StatusBadge({ status }: { status: CblMatchStatus }) {
  if (status === "matched") {
    return (
      <Badge
        variant="secondary"
        className="bg-emerald-500/10 text-emerald-700 dark:text-emerald-400"
      >
        <CheckCircle2 className="mr-1 h-3 w-3" />
        Matched
      </Badge>
    );
  }
  if (status === "manual") {
    return (
      <Badge
        variant="secondary"
        className="bg-emerald-500/10 text-emerald-700 dark:text-emerald-400"
      >
        <Check className="mr-1 h-3 w-3" />
        Manual
      </Badge>
    );
  }
  if (status === "ambiguous") {
    return (
      <Badge
        variant="secondary"
        className="bg-amber-500/10 text-amber-700 dark:text-amber-400"
      >
        <HelpCircle className="mr-1 h-3 w-3" />
        Ambiguous
      </Badge>
    );
  }
  return (
    <Badge
      variant="secondary"
      className="bg-rose-500/10 text-rose-700 dark:text-rose-400"
    >
      <X className="mr-1 h-3 w-3" />
      Missing
    </Badge>
  );
}

function ReadingOrderTab({
  listId,
  stats,
}: {
  listId: string;
  stats: {
    total: number;
    matched: number;
    ambiguous: number;
    missing: number;
    manual: number;
  };
}) {
  const [filter, setFilter] = React.useState<"all" | CblMatchStatus>("all");
  // Server-side status filter — chip click triggers a new infinite
  // query, no client-side `.filter()` over a finite array.
  const query = useCblListEntriesInfinite(listId, {
    status: filter === "all" ? undefined : filter,
  });
  const items = React.useMemo<CblEntryHydratedView[]>(
    () => query.data?.pages.flatMap((p) => p.items) ?? [],
    [query.data],
  );
  // Page-1 total is the server's COUNT over the filter set; while
  // loading, fall back to the matching stat from the list-level
  // aggregate counts so the table frame isn't blank.
  const filterTotal =
    query.data?.pages[0]?.total ??
    (filter === "all"
      ? stats.total
      : filter === "matched"
        ? stats.matched
        : filter === "ambiguous"
          ? stats.ambiguous
          : filter === "missing"
            ? stats.missing
            : stats.manual);

  const parentRef = React.useRef<HTMLDivElement>(null);
  const virtualizer = useVirtualizer({
    count: items.length,
    getScrollElement: () => parentRef.current,
    estimateSize: () => 56,
    overscan: 8,
  });

  // Auto-fetch when the last virtual row is near the end of the loaded
  // window. Matches the IntersectionObserver pattern other infinite
  // surfaces use, but works inside the virtualizer's range model.
  const virtualItems = virtualizer.getVirtualItems();
  const lastIndex = virtualItems[virtualItems.length - 1]?.index ?? -1;
  React.useEffect(() => {
    if (
      query.hasNextPage &&
      !query.isFetchingNextPage &&
      lastIndex >= items.length - 5
    ) {
      void query.fetchNextPage();
    }
  }, [lastIndex, items.length, query]);

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-3 pt-1">
      <StatsCard stats={stats} />
      <div className="flex flex-wrap gap-2">
        {STATUS_FILTERS.map((f) => (
          <Button
            key={f.value}
            type="button"
            size="sm"
            variant={filter === f.value ? "default" : "outline"}
            onClick={() => setFilter(f.value)}
          >
            {f.label}
          </Button>
        ))}
      </div>
      {/* Frame the table so its sticky header sits above the
          virtualizer scroll surface. The frame fills remaining
          column height; only the `parentRef` div scrolls. */}
      <div className="border-border flex min-h-0 flex-1 flex-col overflow-hidden rounded-md border">
        <div className="text-muted-foreground bg-muted/40 grid shrink-0 grid-cols-[60px_1fr_60px_60px_120px_60px] gap-2 px-3 py-2 text-xs font-medium tracking-wider uppercase">
          <div>#</div>
          <div>Series</div>
          <div>Issue</div>
          <div>Year</div>
          <div>Status</div>
          <div className="text-right">Match</div>
        </div>
        <div ref={parentRef} className="min-h-0 flex-1 overflow-auto">
          {query.isLoading ? (
            <div className="text-muted-foreground flex items-center gap-2 p-4 text-sm">
              <Loader2 className="h-4 w-4 animate-spin" /> Loading entries…
            </div>
          ) : items.length === 0 ? (
            <div className="text-muted-foreground p-4 text-sm">
              {filter === "all"
                ? "No entries in this list yet."
                : `No ${filter} entries.`}
            </div>
          ) : (
            <div
              style={{
                height: `${virtualizer.getTotalSize()}px`,
                position: "relative",
                width: "100%",
              }}
            >
              {virtualItems.map((vi) => {
                const entry = items[vi.index];
                return (
                  <div
                    key={entry.id}
                    className="border-border absolute top-0 left-0 grid w-full grid-cols-[60px_1fr_60px_60px_120px_60px] items-center gap-2 border-b px-3 text-sm"
                    style={{
                      height: `${vi.size}px`,
                      transform: `translateY(${vi.start}px)`,
                    }}
                  >
                    <div className="text-muted-foreground">
                      {entry.position + 1}
                    </div>
                    <div className="truncate" title={entry.series_name}>
                      {entry.series_name}
                    </div>
                    <div className="font-mono text-xs">
                      #{entry.issue_number}
                    </div>
                    <div className="text-muted-foreground text-xs">
                      {entry.year ?? "—"}
                    </div>
                    <div>
                      <StatusBadge status={entry.match_status} />
                    </div>
                    <div className="text-right">
                      <ManualMatchPopover
                        listId={listId}
                        entry={entry}
                        trigger={
                          <Button type="button" size="sm" variant="ghost">
                            <Pencil className="h-3 w-3" />
                          </Button>
                        }
                      />
                    </div>
                  </div>
                );
              })}
            </div>
          )}
        </div>
        {/* Footer row: visible count vs. total for the active filter,
         *  plus a spinner when paging in more rows. */}
        <div className="text-muted-foreground bg-muted/20 flex shrink-0 items-center justify-between border-t px-3 py-1.5 text-xs">
          <span>
            {items.length} of {filterTotal} loaded
          </span>
          {query.isFetchingNextPage ? (
            <span className="flex items-center gap-1.5">
              <Loader2 className="h-3 w-3 animate-spin" /> Loading more…
            </span>
          ) : null}
        </div>
      </div>
    </div>
  );
}

type ResolutionFilter = "both" | "ambiguous" | "missing";

const RESOLUTION_FILTERS: { value: ResolutionFilter; label: string }[] = [
  { value: "both", label: "All bad matches" },
  { value: "ambiguous", label: "Ambiguous" },
  { value: "missing", label: "Missing" },
];

function ResolutionTab({
  listId,
  stats,
}: {
  listId: string;
  stats: {
    ambiguous: number;
    missing: number;
  };
}) {
  const [filter, setFilter] = React.useState<ResolutionFilter>("both");
  // Server filter — Resolution tab never touches matched rows. A
  // 50k-entry list with 5 bad matches is just two pages here.
  const query = useCblListEntriesInfinite(listId, {
    status:
      filter === "both"
        ? "ambiguous,missing"
        : filter === "ambiguous"
          ? "ambiguous"
          : "missing",
  });
  const items = React.useMemo<CblEntryHydratedView[]>(
    () => query.data?.pages.flatMap((p) => p.items) ?? [],
    [query.data],
  );
  const filterTotal =
    query.data?.pages[0]?.total ??
    (filter === "both"
      ? stats.ambiguous + stats.missing
      : filter === "ambiguous"
        ? stats.ambiguous
        : stats.missing);

  // IntersectionObserver sentinel — works because Resolution scrolls
  // its outer TabsContent rather than running a virtualizer.
  const sentinelRef = React.useRef<HTMLDivElement | null>(null);
  React.useEffect(() => {
    const el = sentinelRef.current;
    if (!el) return;
    const obs = new IntersectionObserver(
      (entries) => {
        if (entries.some((e) => e.isIntersecting)) {
          if (query.hasNextPage && !query.isFetchingNextPage) {
            void query.fetchNextPage();
          }
        }
      },
      { rootMargin: "400px" },
    );
    obs.observe(el);
    return () => obs.disconnect();
  }, [query]);

  if (stats.ambiguous + stats.missing === 0) {
    return (
      <div className="text-muted-foreground rounded-md border border-dashed p-6 text-sm">
        Nothing to resolve — every entry matched.
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-wrap gap-2">
        {RESOLUTION_FILTERS.map((f) => (
          <Button
            key={f.value}
            type="button"
            size="sm"
            variant={filter === f.value ? "default" : "outline"}
            onClick={() => setFilter(f.value)}
          >
            {f.label}
            <Badge variant="secondary" className="ml-2 px-1.5 py-0 text-xs">
              {f.value === "both"
                ? stats.ambiguous + stats.missing
                : f.value === "ambiguous"
                  ? stats.ambiguous
                  : stats.missing}
            </Badge>
          </Button>
        ))}
      </div>
      {query.isLoading ? (
        <div className="text-muted-foreground flex items-center gap-2 py-6 text-sm">
          <Loader2 className="h-4 w-4 animate-spin" /> Loading entries…
        </div>
      ) : items.length === 0 ? (
        <div className="text-muted-foreground rounded-md border border-dashed p-6 text-sm">
          {filter === "ambiguous"
            ? "No ambiguous matches."
            : filter === "missing"
              ? "No missing entries."
              : "Nothing to resolve."}
        </div>
      ) : (
        <ul className="divide-border border-border divide-y rounded-md border">
          {items.map((entry) => (
            <li key={entry.id} className="p-3">
              <ResolutionRow listId={listId} entry={entry} />
            </li>
          ))}
        </ul>
      )}
      <div
        ref={sentinelRef}
        aria-hidden
        className={cn("h-12", query.hasNextPage ? "" : "hidden")}
      />
      <div className="text-muted-foreground text-xs">
        {items.length} of {filterTotal} loaded
        {query.isFetchingNextPage ? " · loading more…" : ""}
      </div>
    </div>
  );
}

function ResolutionRow({
  listId,
  entry,
}: {
  listId: string;
  entry: CblEntryHydratedView;
}) {
  return (
    <div className="flex items-center justify-between gap-3">
      <div className="min-w-0">
        <div className="flex items-center gap-2">
          <StatusBadge status={entry.match_status} />
          <span className="text-muted-foreground text-xs">
            #{entry.position + 1}
          </span>
        </div>
        <div className="truncate font-medium">{entry.series_name}</div>
        <div className="text-muted-foreground text-xs">
          #{entry.issue_number}
          {entry.year ? ` · ${entry.year}` : ""}
          {entry.volume ? ` · vol ${entry.volume}` : ""}
        </div>
      </div>
      <ManualMatchPopover
        listId={listId}
        entry={entry}
        trigger={
          <Button type="button" size="sm" variant="outline">
            Match…
          </Button>
        }
      />
    </div>
  );
}

function HistoryTab({ listId, list }: { listId: string; list: CblDetailView }) {
  // 100 = server's clamp (cbl_lists.rs:1247 `unwrap_or(20).min(100)`).
  // Refresh logs decay in usefulness — paginating them isn't worth the
  // UI cost; we just ask for the entire window the server will hand back.
  const log = useCblRefreshLog(listId, { limit: 100 });
  const refresh = useRefreshCblList(listId);

  if (log.isLoading) {
    return (
      <div className="text-muted-foreground flex items-center gap-2 py-6 text-sm">
        <Loader2 className="h-4 w-4 animate-spin" /> Loading history…
      </div>
    );
  }
  const items = log.data?.items ?? [];
  return (
    <div className="flex flex-col gap-3">
      <div className="flex items-center justify-between">
        <p className="text-muted-foreground text-sm">
          Refresh runs persist a structural diff. Manual matches survive across
          refreshes.
        </p>
        {list.source_kind !== "upload" ? (
          <Button
            type="button"
            size="sm"
            onClick={() => refresh.mutate({})}
            disabled={refresh.isPending}
          >
            <RefreshCw className="mr-1 h-3 w-3" /> Refresh now
          </Button>
        ) : null}
      </div>
      {items.length === 0 ? (
        <div className="text-muted-foreground rounded-md border border-dashed p-6 text-center text-sm">
          No refresh runs yet.
        </div>
      ) : (
        <ul className="divide-border border-border divide-y rounded-md border">
          {items.map((entry) => (
            <li key={entry.id} className="p-3">
              <RefreshRow entry={entry} />
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

function RefreshRow({ entry }: { entry: RefreshLogEntryView }) {
  return (
    <div className="flex flex-col gap-1">
      <div className="flex items-center justify-between gap-2 text-sm">
        <div className="font-medium">
          {new Date(entry.ran_at).toLocaleString()}
        </div>
        <Badge variant="outline" className="text-xs">
          {entry.trigger}
        </Badge>
      </div>
      <div className="text-muted-foreground text-xs">
        {entry.upstream_changed ? "Upstream changed" : "Upstream unchanged"} · +
        {entry.added_count} / -{entry.removed_count} / ↻{entry.reordered_count}
        {entry.rematched_count > 0
          ? ` · ${entry.rematched_count} re-matched`
          : ""}
      </div>
    </div>
  );
}

function SettingsTab({
  list,
  savedView,
}: {
  list: CblDetailView;
  savedView: SavedViewView;
}) {
  const router = useRouter();
  const deleteList = useDeleteCblList(list.id);
  const deleteView = useDeleteSavedView(savedView.id);
  const [editOpen, setEditOpen] = React.useState(false);
  const [hideMissing, setHideMissing] = useCblHideMissing(list.id);

  async function deleteEverything() {
    // Saved view first so the cbl_list isn't orphaned mid-flight.
    await deleteView.mutateAsync();
    await deleteList.mutateAsync();
    router.push("/");
  }

  return (
    <div className="flex max-w-xl flex-col gap-4">
      <div>
        <Button
          type="button"
          variant="outline"
          size="sm"
          onClick={() => setEditOpen(true)}
        >
          <Pencil className="mr-1 h-4 w-4" /> Edit metadata
        </Button>
        <p className="text-muted-foreground mt-2 text-xs">
          Edits the saved-view name, description, tags, year overlay, and
          refresh schedule. Entries themselves stay sourced from the imported
          `.cbl` file.
        </p>
      </div>
      <div className="flex items-start justify-between gap-4 rounded-md border p-3">
        <div className="min-w-0">
          <Label
            htmlFor={`hide-missing-${list.id}`}
            className="text-sm font-medium"
          >
            Hide missing entries
          </Label>
          <p className="text-muted-foreground mt-1 text-xs">
            On the consumption view, skip entries that aren&apos;t in your
            library. CBL position numbers stay accurate and a small gap
            marker shows where missing entries were. Saved per list, this
            device.
          </p>
        </div>
        <Switch
          id={`hide-missing-${list.id}`}
          checked={hideMissing}
          onCheckedChange={setHideMissing}
        />
      </div>
      <dl className="text-muted-foreground grid grid-cols-[8rem_1fr] gap-y-1 text-sm">
        <dt>Source kind</dt>
        <dd className="text-foreground">{list.source_kind}</dd>
        {list.source_url ? (
          <>
            <dt>Source URL</dt>
            <dd className="text-foreground truncate font-mono text-xs">
              {list.source_url}
            </dd>
          </>
        ) : null}
        {list.catalog_path ? (
          <>
            <dt>Catalog path</dt>
            <dd className="text-foreground truncate font-mono text-xs">
              {list.catalog_path}
            </dd>
          </>
        ) : null}
        <dt>Refresh schedule</dt>
        <dd className="text-foreground">{list.refresh_schedule ?? "manual"}</dd>
        {list.last_refreshed_at ? (
          <>
            <dt>Last refreshed</dt>
            <dd className="text-foreground">
              {new Date(list.last_refreshed_at).toLocaleString()}
            </dd>
          </>
        ) : null}
      </dl>
      <div className="flex items-center justify-end border-t pt-4">
        <AlertDialog>
          <AlertDialogTrigger asChild>
            <Button type="button" variant="destructive">
              <Trash2 className="mr-1 h-4 w-4" />
              Delete
            </Button>
          </AlertDialogTrigger>
          <AlertDialogContent>
            <AlertDialogHeader>
              <AlertDialogTitle>Delete this CBL view?</AlertDialogTitle>
              <AlertDialogDescription>
                This removes the saved view and the underlying CBL list. Manual
                matches you have recorded will be lost.
              </AlertDialogDescription>
            </AlertDialogHeader>
            <AlertDialogFooter>
              <AlertDialogCancel>Cancel</AlertDialogCancel>
              <AlertDialogAction onClick={deleteEverything}>
                Delete
              </AlertDialogAction>
            </AlertDialogFooter>
          </AlertDialogContent>
        </AlertDialog>
      </div>
      <EditCblMetadataDialog
        view={savedView}
        list={list}
        open={editOpen}
        onOpenChange={setEditOpen}
      />
    </div>
  );
}
