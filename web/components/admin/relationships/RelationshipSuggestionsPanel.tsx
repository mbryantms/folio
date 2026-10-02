"use client";

/**
 * Relationship-suggestion review queue (WP-7.3, spec §5.7).
 *
 * `GET /admin/relationship-suggestions`, cursor-paginated; status, bucket
 * and library are all **server** params, and the next page loads when the
 * sentinel scrolls into view (IssuesPanel template). `bucket_counts` and
 * `total` ride on the first page.
 *
 * Per row: accept, accept as a different kind ("Edit kind"), reject, and —
 * in the rejected view — reopen. Pending rows can be multi-selected for a
 * bulk accept / reject (one batch, one audit row), and "Accept all high"
 * takes the pending high-confidence rows (up to 500 per request) behind an
 * AlertDialog.
 */

import {
  Check,
  ChevronDown,
  Loader2,
  Play,
  RotateCcw,
  Sparkles,
  X,
} from "lucide-react";
import Link from "next/link";
import * as React from "react";

import { Cover } from "@/components/Cover";
import { SelectionToolbar } from "@/components/library/SelectionToolbar";
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
import { Checkbox } from "@/components/ui/checkbox";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
import { FilterPill } from "@/components/ui/filter-pill";
import { NativeSelect } from "@/components/ui/native-select";
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
import { Skeleton } from "@/components/ui/skeleton";
import {
  useAcceptRelationshipSuggestion,
  useBulkAcceptRelationshipSuggestions,
  useBulkRejectRelationshipSuggestions,
  useRejectRelationshipSuggestion,
  useReopenRelationshipSuggestion,
  useRunRelationshipSuggestions,
} from "@/lib/api/mutations";
import {
  useLibraryList,
  useRelationshipSuggestionsInfinite,
} from "@/lib/api/queries";
import type {
  RelationshipKind,
  RelationshipSuggestionView,
  SuggestionBucket,
  SuggestionStatus,
  SuggestionStatusFilter,
} from "@/lib/api/types";
import { useSelection } from "@/lib/selection/use-selection";
import { kindLabel, RELATIONSHIP_KINDS } from "@/lib/relationships";
import { seriesUrl } from "@/lib/urls";
import { cn } from "@/lib/utils";

export const STATUS_FILTERS: {
  value: SuggestionStatusFilter;
  label: string;
}[] = [
  { value: "pending", label: "Pending" },
  { value: "accepted", label: "Accepted" },
  { value: "modified", label: "Modified" },
  { value: "rejected", label: "Rejected" },
  { value: "stale", label: "Stale" },
  { value: "all", label: "All" },
];

export const BUCKET_FILTERS: { value: SuggestionBucket; label: string }[] = [
  { value: "high", label: "High" },
  { value: "medium", label: "Medium" },
  { value: "low", label: "Low" },
];

const STATUS_LABEL: Record<SuggestionStatus, string> = {
  pending: "Pending",
  accepted: "Accepted",
  modified: "Accepted (modified)",
  rejected: "Rejected",
  stale: "Stale",
};

/** Display name for an evidence `source` key (`story_arc` → "Story arc"). */
export function sourceLabel(source: string): string {
  const s = source.replace(/_/g, " ");
  return s.charAt(0).toUpperCase() + s.slice(1);
}

type EvidenceSource = {
  source?: string;
  confidence?: number;
  reason?: string;
  [k: string]: unknown;
};

/** `evidence.sources[]`, tolerating any shape the server stored. */
export function evidenceSources(evidence: unknown): EvidenceSource[] {
  if (!evidence || typeof evidence !== "object") return [];
  const sources = (evidence as { sources?: unknown }).sources;
  return Array.isArray(sources)
    ? sources.filter((s): s is EvidenceSource => !!s && typeof s === "object")
    : [];
}

function formatValue(v: unknown): string {
  if (v == null) return "—";
  if (Array.isArray(v)) return v.map(formatValue).join(", ");
  if (typeof v === "object") return JSON.stringify(v);
  return String(v);
}

function pct(c: number): string {
  return `${Math.round(c * 100)}%`;
}

export function RelationshipSuggestionsPanel() {
  const libraries = useLibraryList();
  const [status, setStatus] = React.useState<SuggestionStatusFilter>("pending");
  const [bucket, setBucket] = React.useState<SuggestionBucket | null>(null);
  const [libraryId, setLibraryId] = React.useState<string | null>(null);
  const [confirmHigh, setConfirmHigh] = React.useState(false);
  const [confirmBulkReject, setConfirmBulkReject] = React.useState(false);

  const query = useRelationshipSuggestionsInfinite({
    status,
    bucket,
    libraryId,
  });
  const { data, isLoading, error, hasNextPage, isFetchingNextPage } = query;
  const fetchNextPage = query.fetchNextPage;
  const sentinelRef = React.useRef<HTMLDivElement | null>(null);

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

  const items = React.useMemo(
    () => data?.pages.flatMap((p) => p.items) ?? [],
    [data],
  );
  const first = data?.pages[0];
  const counts = first?.bucket_counts;
  const total = first?.total ?? items.length;

  const selection = useSelection(items);
  const bulkAccept = useBulkAcceptRelationshipSuggestions();
  const bulkReject = useBulkRejectRelationshipSuggestions();
  const run = useRunRelationshipSuggestions();
  const bulkPending = bulkAccept.isPending || bulkReject.isPending;
  const selectedIds = [...selection.selected];

  const libraryList = libraries.data ?? [];
  const libraryName = libraryId
    ? (libraryList.find((l) => l.id === libraryId)?.name ?? "this library")
    : null;
  const highCount = counts?.high ?? 0;

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-center gap-3">
        <label className="flex items-center gap-2 text-sm">
          <span className="text-muted-foreground">Library</span>
          <NativeSelect
            size="sm"
            aria-label="Library"
            value={libraryId ?? ""}
            onChange={(v) => {
              setLibraryId(v || null);
              selection.exit();
            }}
            options={[
              { value: "", label: "All libraries" },
              ...libraryList.map((l) => ({ value: l.id, label: l.name })),
            ]}
          />
        </label>
        <Button
          size="sm"
          variant="outline"
          disabled={run.isPending}
          onClick={() => run.mutate({ libraryId })}
        >
          {run.isPending ? (
            <Loader2 className="mr-1 size-3.5 animate-spin" />
          ) : (
            <Play className="mr-1 size-3.5" />
          )}
          {libraryName
            ? `Run now for ${libraryName}`
            : "Run now (all libraries)"}
        </Button>
        {status === "pending" && highCount > 0 ? (
          <Button
            size="sm"
            className="ml-auto"
            disabled={bulkPending}
            onClick={() => setConfirmHigh(true)}
          >
            <Sparkles className="mr-1 size-3.5" />
            Accept all high-confidence ({highCount.toLocaleString()})
          </Button>
        ) : null}
      </div>

      <div className="space-y-2">
        <div
          className="flex flex-wrap items-center gap-2"
          role="group"
          aria-label="Status"
        >
          {STATUS_FILTERS.map((f) => (
            <FilterPill
              key={f.value}
              active={status === f.value}
              onClick={() => {
                setStatus(f.value);
                selection.exit();
              }}
            >
              {f.label}
            </FilterPill>
          ))}
        </div>
        <div
          className="flex flex-wrap items-center gap-2"
          role="group"
          aria-label="Confidence"
        >
          <FilterPill
            active={bucket === null}
            onClick={() => {
              setBucket(null);
              selection.exit();
            }}
            count={
              counts ? counts.high + counts.medium + counts.low : undefined
            }
          >
            Any confidence
          </FilterPill>
          {BUCKET_FILTERS.map((f) => (
            <FilterPill
              key={f.value}
              active={bucket === f.value}
              onClick={() => {
                setBucket(f.value);
                selection.exit();
              }}
              count={counts?.[f.value]}
            >
              {f.label}
            </FilterPill>
          ))}
          {status === "pending" && items.length > 0 && !selection.selectMode ? (
            <Button
              size="sm"
              variant="ghost"
              className="ml-auto"
              onClick={() => selection.enter()}
            >
              Select…
            </Button>
          ) : null}
        </div>
      </div>

      <SelectionToolbar
        open={selection.selectMode}
        count={selection.count}
        total={items.length}
        primary={[
          {
            id: "accept",
            label: "Accept",
            icon: Check,
            onClick: () =>
              bulkAccept.mutate(
                { ids: selectedIds },
                { onSuccess: () => selection.exit() },
              ),
            disabled: bulkPending || selection.count === 0,
          },
          {
            id: "reject",
            label: "Reject",
            icon: X,
            onClick: () => setConfirmBulkReject(true),
            disabled: bulkPending || selection.count === 0,
            destructive: true,
          },
        ]}
        onDone={() => selection.exit()}
        onClear={() => selection.clear()}
        onSelectAll={() => selection.selectAll()}
        isPending={bulkPending}
      />

      {isLoading ? (
        <Skeleton className="h-64 w-full" />
      ) : error ? (
        <p className="text-destructive text-sm">{error.message}</p>
      ) : items.length === 0 ? (
        <p className="border-border bg-card/40 text-muted-foreground rounded-md border border-dashed px-4 py-12 text-center text-sm">
          {status === "pending"
            ? "No pending suggestions. Run the engine to look for new ones."
            : "Nothing here."}
        </p>
      ) : (
        <>
          <p className="text-muted-foreground text-xs">
            {total.toLocaleString()} suggestion{total === 1 ? "" : "s"}
          </p>
          <ul className="space-y-3">
            {items.map((s) => (
              <SuggestionRow
                key={s.id}
                suggestion={s}
                selectable={selection.selectMode && s.status === "pending"}
                selected={selection.isSelected(s.id)}
                onToggle={(ev) => selection.toggle(s.id, ev)}
              />
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

      <AlertDialog open={confirmHigh} onOpenChange={setConfirmHigh}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              Accept all high-confidence suggestions?
            </AlertDialogTitle>
            <AlertDialogDescription>
              Links {highCount.toLocaleString()} pending high-confidence
              suggestion{highCount === 1 ? "" : "s"}
              {libraryName ? ` in ${libraryName}` : " across every library"} as
              relationships (up to 500 per run; run it again for the rest).
              Suggestions that conflict with an existing relationship are
              skipped and stay pending. Each link can be removed from the series
              page.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              onClick={() =>
                bulkAccept.mutate(
                  libraryId
                    ? { bucket: "high", library_id: libraryId }
                    : { bucket: "high" },
                )
              }
            >
              Accept all
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      <AlertDialog open={confirmBulkReject} onOpenChange={setConfirmBulkReject}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              Reject {selection.count} suggestion
              {selection.count === 1 ? "" : "s"}?
            </AlertDialogTitle>
            <AlertDialogDescription>
              Rejected suggestions are never proposed again unless you reopen
              them from the Rejected view.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              onClick={() =>
                bulkReject.mutate(
                  { ids: selectedIds },
                  { onSuccess: () => selection.exit() },
                )
              }
            >
              Reject
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}

function SeriesEnd({
  series,
}: {
  series: RelationshipSuggestionView["from_series"];
}) {
  return (
    <Link
      href={seriesUrl(series)}
      className="group flex min-w-0 items-center gap-2"
    >
      <div className="w-10 shrink-0">
        <Cover
          src={series.cover_url}
          alt={series.name}
          fallback={series.publisher}
        />
      </div>
      <span className="min-w-0">
        <span className="line-clamp-2 text-sm font-medium group-hover:underline">
          {series.name}
        </span>
        {series.year != null ? (
          <span className="text-muted-foreground block text-xs">
            {series.year}
          </span>
        ) : null}
      </span>
    </Link>
  );
}

function SuggestionRow({
  suggestion: s,
  selectable,
  selected,
  onToggle,
}: {
  suggestion: RelationshipSuggestionView;
  selectable: boolean;
  selected: boolean;
  onToggle: (ev?: { shiftKey?: boolean }) => void;
}) {
  const accept = useAcceptRelationshipSuggestion();
  const reject = useRejectRelationshipSuggestion();
  const reopen = useReopenRelationshipSuggestion();
  const busy = accept.isPending || reject.isPending || reopen.isPending;
  const sources = evidenceSources(s.evidence);
  const acceptedKind = s.accepted_kind ? kindLabel(s.accepted_kind) : null;

  return (
    <li
      className={cn(
        "border-border bg-card rounded-md border",
        selected && "ring-primary ring-2",
      )}
      data-testid="relationship-suggestion"
      data-suggestion-id={s.id}
    >
      <div className="flex flex-wrap items-start gap-3 px-4 py-3">
        {selectable ? (
          <Checkbox
            className="mt-3"
            checked={selected}
            aria-label={`Select ${s.from_series.name} ${s.kind_label.toLowerCase()} ${s.to_series.name}`}
            onClick={(e) => {
              e.preventDefault();
              onToggle({ shiftKey: e.shiftKey });
            }}
          />
        ) : null}
        <div className="grid min-w-0 flex-1 gap-2 sm:grid-cols-[minmax(0,1fr)_auto_minmax(0,1fr)] sm:items-center">
          <SeriesEnd series={s.from_series} />
          <div className="flex flex-col items-start gap-1 sm:items-center">
            <Badge variant="secondary">{s.kind_label}</Badge>
            {acceptedKind ? (
              <span className="text-muted-foreground text-xs">
                accepted as {acceptedKind.toLowerCase()}
              </span>
            ) : null}
          </div>
          <SeriesEnd series={s.to_series} />
        </div>
        <div className="flex shrink-0 flex-col items-end gap-1 text-xs">
          <span className="text-foreground font-medium tabular-nums">
            {pct(s.confidence)}
          </span>
          <Badge variant="outline" className="capitalize">
            {s.bucket}
          </Badge>
          {s.status !== "pending" ? (
            <Badge
              variant={s.status === "rejected" ? "destructive" : "secondary"}
            >
              {STATUS_LABEL[s.status]}
            </Badge>
          ) : null}
        </div>
      </div>
      <div className="border-border space-y-2 border-t px-4 py-3">
        <p className="text-muted-foreground text-sm">{s.reason}</p>
        <div className="flex flex-wrap items-center gap-2">
          {sources.length > 0 ? (
            <Collapsible className="w-full sm:w-auto sm:flex-1">
              <CollapsibleTrigger asChild>
                <Button
                  variant="ghost"
                  size="sm"
                  className="group text-muted-foreground -ml-2"
                >
                  <ChevronDown className="mr-1 size-3.5 transition-transform group-data-[state=open]:rotate-180" />
                  Evidence ({sources.length} source
                  {sources.length === 1 ? "" : "s"})
                </Button>
              </CollapsibleTrigger>
              <CollapsibleContent>
                <ul className="mt-2 space-y-2">
                  {sources.map((src, i) => (
                    <li
                      key={`${src.source ?? "source"}-${i}`}
                      className="bg-muted/40 rounded-md px-3 py-2 text-xs"
                    >
                      <p className="text-foreground font-medium">
                        {sourceLabel(src.source ?? "unknown")}
                        {typeof src.confidence === "number"
                          ? ` · ${pct(src.confidence)}`
                          : ""}
                      </p>
                      {src.reason ? (
                        <p className="text-muted-foreground">{src.reason}</p>
                      ) : null}
                      <dl className="text-muted-foreground mt-1 grid grid-cols-[auto_1fr] gap-x-3">
                        {Object.entries(src)
                          .filter(
                            ([k]) =>
                              k !== "source" &&
                              k !== "confidence" &&
                              k !== "reason",
                          )
                          .map(([k, v]) => (
                            <React.Fragment key={k}>
                              <dt>{sourceLabel(k)}</dt>
                              <dd className="wrap-anywhere">
                                {formatValue(v)}
                              </dd>
                            </React.Fragment>
                          ))}
                      </dl>
                    </li>
                  ))}
                </ul>
              </CollapsibleContent>
            </Collapsible>
          ) : null}
          <div className="ml-auto flex items-center gap-1">
            {s.status === "pending" ? (
              <>
                <Button
                  size="sm"
                  disabled={busy}
                  onClick={() => accept.mutate({ id: s.id })}
                >
                  <Check className="mr-1 size-3.5" />
                  Accept
                </Button>
                <EditKindPopover
                  suggestion={s}
                  disabled={busy}
                  onAccept={(kind) => accept.mutate({ id: s.id, kind })}
                />
                <Button
                  size="sm"
                  variant="ghost"
                  disabled={busy}
                  onClick={() => reject.mutate({ id: s.id })}
                >
                  <X className="mr-1 size-3.5" />
                  Reject
                </Button>
              </>
            ) : s.status === "rejected" ? (
              <Button
                size="sm"
                variant="outline"
                disabled={busy}
                onClick={() => reopen.mutate({ id: s.id })}
              >
                <RotateCcw className="mr-1 size-3.5" />
                Reopen
              </Button>
            ) : s.reviewed_at ? (
              <span className="text-muted-foreground text-xs">
                Reviewed {new Date(s.reviewed_at).toLocaleDateString()}
              </span>
            ) : null}
          </div>
        </div>
      </div>
    </li>
  );
}

/** "Edit kind": pick any of the nine kinds (read "from `kind` to") and
 *  accept as modified. */
function EditKindPopover({
  suggestion: s,
  disabled,
  onAccept,
}: {
  suggestion: RelationshipSuggestionView;
  disabled: boolean;
  onAccept: (kind: RelationshipKind) => void;
}) {
  const [open, setOpen] = React.useState(false);
  const [kind, setKind] = React.useState<RelationshipKind>(s.kind);
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button size="sm" variant="outline" disabled={disabled}>
          Edit kind
        </Button>
      </PopoverTrigger>
      <PopoverContent align="end" className="w-[min(320px,calc(100vw-2rem))]">
        <div className="space-y-3">
          <p className="text-sm">
            <span className="font-medium">{s.from_series.name}</span> is…
          </p>
          <Select
            value={kind}
            onValueChange={(v) => setKind(v as RelationshipKind)}
          >
            <SelectTrigger aria-label="Relationship kind">
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
          <p className="text-muted-foreground text-sm">
            …
            <span className="text-foreground font-medium">
              {s.to_series.name}
            </span>
          </p>
          <Button
            size="sm"
            className="w-full"
            onClick={() => {
              onAccept(kind);
              setOpen(false);
            }}
          >
            {kind === s.kind ? "Accept" : "Accept as modified"}
          </Button>
        </div>
      </PopoverContent>
    </Popover>
  );
}
