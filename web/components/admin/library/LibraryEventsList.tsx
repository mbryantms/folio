"use client";

import * as React from "react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { useLibraryEventsInfinite } from "@/lib/api/queries";
import type { LibraryEventView } from "@/lib/api/types";

const CATEGORIES = [
  "issue",
  "series",
  "file",
  "cover",
  "thumbnail",
  "metadata",
  "archive",
  "health",
  "scan",
] as const;
const SEVERITIES = ["info", "warning", "error"] as const;

/**
 * Itemized library-event manifest (observability-split M10). Scoped by exactly
 * one of `batchId` / `scanRunId` / `libraryId` (or unscoped for the global
 * activity log). Category + severity chips drive **server-side** query params —
 * never an in-memory filter over a truncated page — so a filtered view never
 * silently drops rows past the page cap.
 */
export function LibraryEventsList(props: {
  batchId?: string;
  scanRunId?: string;
  libraryId?: string;
  /** Show the library name column (cross-library activity log). */
  showLibrary?: boolean;
}) {
  const [categories, setCategories] = React.useState<string[]>([]);
  const [severities, setSeverities] = React.useState<string[]>([]);

  const q = useLibraryEventsInfinite({
    batch_id: props.batchId,
    scan_run_id: props.scanRunId,
    library_id: props.libraryId,
    category: categories.length ? categories.join(",") : undefined,
    severity: severities.length ? severities.join(",") : undefined,
    limit: 50,
  });

  const toggle = (
    set: React.Dispatch<React.SetStateAction<string[]>>,
    value: string,
  ) =>
    set((prev) =>
      prev.includes(value) ? prev.filter((v) => v !== value) : [...prev, value],
    );

  const events = q.data?.pages.flatMap((p) => p.items) ?? [];

  return (
    <div className="space-y-3">
      {/* Filter chips */}
      <div className="flex flex-wrap items-center gap-1.5">
        {SEVERITIES.map((s) => (
          <Chip
            key={s}
            label={s}
            active={severities.includes(s)}
            onClick={() => toggle(setSeverities, s)}
          />
        ))}
        <span className="bg-border mx-1 h-4 w-px" aria-hidden />
        {CATEGORIES.map((c) => (
          <Chip
            key={c}
            label={c}
            active={categories.includes(c)}
            onClick={() => toggle(setCategories, c)}
          />
        ))}
      </div>

      {q.isLoading ? (
        <Skeleton className="h-48 w-full" />
      ) : q.error ? (
        <p className="text-destructive text-sm">Failed to load events.</p>
      ) : events.length === 0 ? (
        <p className="text-muted-foreground text-sm">
          No events recorded for the selected filters.
        </p>
      ) : (
        <ul className="divide-border border-border bg-card divide-y rounded-md border">
          {events.map((e) => (
            <EventRow key={e.id} event={e} showLibrary={props.showLibrary} />
          ))}
        </ul>
      )}

      {q.hasNextPage ? (
        <Button
          type="button"
          variant="outline"
          size="sm"
          onClick={() => q.fetchNextPage()}
          disabled={q.isFetchingNextPage}
        >
          {q.isFetchingNextPage ? "Loading…" : "Load more"}
        </Button>
      ) : null}
    </div>
  );
}

function Chip({
  label,
  active,
  onClick,
}: {
  label: string;
  active: boolean;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      aria-pressed={active}
      className={`rounded-full border px-2.5 py-0.5 text-xs capitalize transition-colors ${
        active
          ? "border-primary bg-primary/15 text-foreground"
          : "border-border text-muted-foreground hover:bg-muted/40"
      }`}
    >
      {label}
    </button>
  );
}

const SEV_DOT: Record<string, string> = {
  info: "bg-muted-foreground/50",
  warning: "bg-amber-400",
  error: "bg-destructive",
};

/** Human label for a thumbnail job's `detail.kind` — what was being made. */
const THUMB_TARGET: Record<string, string> = {
  cover: "Cover image",
  page_map: "Page thumbnails",
  cover_page_map: "Cover + page thumbnails",
};

function asRecord(v: unknown): Record<string, unknown> | null {
  return v && typeof v === "object" && !Array.isArray(v)
    ? (v as Record<string, unknown>)
    : null;
}

function EventRow({
  event,
  showLibrary,
}: {
  event: LibraryEventView;
  showLibrary?: boolean;
}) {
  const detail = asRecord(event.detail);
  const errorMsg = typeof detail?.error === "string" ? detail.error : null;
  const filePath = typeof detail?.path === "string" ? detail.path : null;
  const seriesName = typeof detail?.series === "string" ? detail.series : null;
  const target =
    event.category === "thumbnail" && typeof detail?.kind === "string"
      ? (THUMB_TARGET[detail.kind] ?? detail.kind)
      : null;
  // Only worth expanding when there's structured detail to show.
  const hasDetail = detail != null && Object.keys(detail).length > 0;

  const header = (
    <div className="flex items-start gap-3">
      <span
        className={`mt-1.5 h-2 w-2 shrink-0 rounded-full ${SEV_DOT[event.severity] ?? "bg-muted-foreground/50"}`}
        aria-hidden
      />
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2">
          <Badge variant="outline" className="shrink-0 capitalize">
            {event.category}/{event.action}
          </Badge>
          <span className="truncate">{event.summary}</span>
        </div>
        {showLibrary && event.library_name && (
          <p className="text-muted-foreground mt-0.5 text-xs">
            {event.library_name}
          </p>
        )}
      </div>
      <time className="text-muted-foreground shrink-0 font-mono text-xs">
        {new Date(event.created_at).toLocaleString()}
      </time>
    </div>
  );

  if (!hasDetail) {
    return <li className="px-3 py-2 text-sm">{header}</li>;
  }

  return (
    <li className="px-3 py-2 text-sm">
      <details className="group">
        <summary className="cursor-pointer list-none [&::-webkit-details-marker]:hidden">
          {header}
        </summary>
        <div className="mt-2 space-y-1.5 pl-5">
          {seriesName && (
            <p className="text-xs">
              <span className="text-muted-foreground">Series: </span>
              {seriesName}
            </p>
          )}
          {target && (
            <p className="text-xs">
              <span className="text-muted-foreground">Target: </span>
              {target}
            </p>
          )}
          {errorMsg && (
            <p className="text-destructive text-xs wrap-anywhere">
              <span className="text-muted-foreground">Error: </span>
              {errorMsg}
            </p>
          )}
          {filePath && (
            <p className="text-xs wrap-anywhere">
              <span className="text-muted-foreground">File: </span>
              <span className="font-mono">{filePath}</span>
            </p>
          )}
          {!filePath && event.entity_label && (
            <p className="text-muted-foreground text-xs">
              {event.entity_type ?? "entity"}: {event.entity_label}
            </p>
          )}
          <pre className="bg-muted/40 overflow-x-auto rounded p-2 text-xs">
            {JSON.stringify(event.detail, null, 2)}
          </pre>
        </div>
      </details>
    </li>
  );
}
