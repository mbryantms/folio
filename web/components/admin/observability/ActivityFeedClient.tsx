"use client";

import { AlertTriangle, BookOpen, FileClock, Library } from "lucide-react";
import { useMemo, useState } from "react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { formatDurationMs } from "@/lib/activity";
import { useAdminActivity } from "@/lib/api/queries";
import type { ActivityEntryView, ActivityKind } from "@/lib/api/types";
import { cn } from "@/lib/utils";

const ALL_KINDS: ReadonlyArray<{
  value: ActivityKind;
  label: string;
  icon: React.ReactNode;
}> = [
  {
    value: "audit",
    label: "Audit",
    icon: <FileClock className="h-3.5 w-3.5" />,
  },
  { value: "scan", label: "Scans", icon: <Library className="h-3.5 w-3.5" /> },
  {
    value: "health",
    label: "Health",
    icon: <AlertTriangle className="h-3.5 w-3.5" />,
  },
  {
    value: "reading",
    label: "Reading volume",
    icon: <BookOpen className="h-3.5 w-3.5" />,
  },
];

export function ActivityFeedClient() {
  const [active, setActive] = useState<Set<ActivityKind>>(
    () => new Set(ALL_KINDS.map((k) => k.value)),
  );
  const filters = useMemo(
    () => ({ kinds: [...active] as ActivityKind[], limit: 50 }),
    [active],
  );
  const feed = useAdminActivity(filters);

  function toggle(k: ActivityKind) {
    setActive((prev) => {
      const next = new Set(prev);
      if (next.has(k)) {
        next.delete(k);
      } else {
        next.add(k);
      }
      return next;
    });
  }

  const entries = (feed.data?.pages ?? []).flatMap((p) => p.entries);

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-center gap-2">
        <span className="text-muted-foreground text-xs font-semibold tracking-wide uppercase">
          Show
        </span>
        {ALL_KINDS.map(({ value, label, icon }) => {
          const on = active.has(value);
          return (
            <Button
              key={value}
              type="button"
              size="sm"
              variant={on ? "default" : "outline"}
              onClick={() => toggle(value)}
              aria-pressed={on}
            >
              <span className="mr-1.5">{icon}</span>
              {label}
            </Button>
          );
        })}
      </div>

      {feed.isLoading ? (
        <Skeleton className="h-64 w-full" />
      ) : feed.error ? (
        <p className="text-destructive text-sm">
          Failed to load activity feed.
        </p>
      ) : entries.length === 0 ? (
        <p className="text-muted-foreground text-sm">
          No activity recorded for the selected filters yet.
        </p>
      ) : (
        <ul className="divide-border border-border bg-card divide-y rounded-md border">
          {entries.map((entry) => (
            <ActivityRow
              key={`${entry.kind}-${entry.source_id}`}
              entry={entry}
            />
          ))}
        </ul>
      )}

      {feed.hasNextPage ? (
        <Button
          type="button"
          variant="outline"
          size="sm"
          onClick={() => feed.fetchNextPage()}
          disabled={feed.isFetchingNextPage}
        >
          {feed.isFetchingNextPage ? "Loading…" : "Load older"}
        </Button>
      ) : null}
    </div>
  );
}

function ActivityRow({ entry }: { entry: ActivityEntryView }) {
  const meta = ALL_KINDS.find((k) => k.value === entry.kind);
  const detail = formatDetail(entry);
  return (
    <li className="grid grid-cols-[8rem_1fr] items-baseline gap-3 px-3 py-2.5 text-sm">
      <div className="text-muted-foreground flex items-center gap-2 text-xs">
        <KindBadge kind={entry.kind} />
        <time className="font-mono">{formatTime(entry.timestamp)}</time>
      </div>
      <div className="min-w-0">
        <p className="text-foreground">
          {meta ? (
            <span className="text-muted-foreground mr-1.5 align-middle">
              {meta.icon}
            </span>
          ) : null}
          {entry.summary}
        </p>
        {detail ? (
          <p className="text-muted-foreground mt-0.5 text-xs">{detail}</p>
        ) : null}
      </div>
    </li>
  );
}

function KindBadge({ kind }: { kind: string }) {
  const tone = KIND_TONE[kind] ?? "border-border text-muted-foreground";
  return (
    <Badge variant="outline" className={cn("font-mono text-[10px]", tone)}>
      {kind}
    </Badge>
  );
}

const KIND_TONE: Record<string, string> = {
  audit: "border-violet-500/40 text-violet-300",
  scan: "border-sky-500/40 text-sky-400",
  health: "border-amber-500/40 text-amber-300",
  reading: "border-emerald-500/40 text-emerald-400",
};

function formatDetail(entry: ActivityEntryView): string | null {
  switch (entry.kind) {
    case "audit": {
      const actor = (entry.payload.actor_type as string) ?? "user";
      const target = entry.payload.target_type as string | null;
      const targetId = entry.payload.target_id as string | null;
      const tail =
        target && targetId ? ` · ${target} ${targetId.slice(0, 12)}…` : "";
      return `${actor}${tail}`;
    }
    case "scan": {
      const lib = entry.payload.library_id as string | undefined;
      const err = entry.payload.error as string | null | undefined;
      const tail = lib ? `library ${lib.slice(0, 8)}…` : "";
      return err ? `${tail} · ${err}` : tail || null;
    }
    case "health": {
      const lib = entry.payload.library_id as string | undefined;
      return lib ? `library ${lib.slice(0, 8)}…` : null;
    }
    case "reading": {
      const ms = numberOr(entry.payload.active_ms, 0);
      const pages = numberOr(entry.payload.pages, 0);
      return `${formatDurationMs(ms)} · ${pages} pages`;
    }
    default:
      return null;
  }
}

function numberOr(v: unknown, fallback: number): number {
  return typeof v === "number" ? v : fallback;
}

function formatTime(iso: string): string {
  const d = new Date(iso);
  return d.toLocaleTimeString(undefined, {
    hour: "numeric",
    minute: "2-digit",
  });
}
