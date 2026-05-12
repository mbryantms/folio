"use client";

import { Pause, Play } from "lucide-react";
import { useMemo, useState } from "react";

import { SegmentedControl } from "@/components/settings/SegmentedControl";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import { useAdminLogs } from "@/lib/api/queries";
import type { LogEntryView, LogLevel } from "@/lib/api/types";
import { cn } from "@/lib/utils";

const LEVELS: ReadonlyArray<{ value: LogLevel | "all"; label: string }> = [
  { value: "all", label: "All" },
  { value: "error", label: "Error" },
  { value: "warn", label: "Warn" },
  { value: "info", label: "Info" },
  { value: "debug", label: "Debug" },
];

export function LogsClient() {
  const [level, setLevel] = useState<LogLevel | "all">("info");
  const [q, setQ] = useState("");
  const [tail, setTail] = useState(true);

  const filters = useMemo(
    () => ({
      level: level === "all" ? undefined : level,
      q: q.trim() || undefined,
      limit: 500,
    }),
    [level, q],
  );

  const logs = useAdminLogs(filters, { intervalMs: tail ? 2_000 : undefined });

  return (
    <div className="space-y-4">
      <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
        <div className="flex flex-wrap items-center gap-3">
          <SegmentedControl
            value={level}
            onChange={setLevel}
            options={LEVELS}
            ariaLabel="Log level"
          />
          <Input
            type="search"
            placeholder="Filter by message, target, or field…"
            value={q}
            onChange={(e) => setQ(e.currentTarget.value)}
            className="h-9 w-72"
          />
        </div>
        <div className="flex items-center gap-2">
          <Button
            type="button"
            size="sm"
            variant={tail ? "default" : "outline"}
            onClick={() => setTail((v) => !v)}
          >
            {tail ? (
              <Pause className="mr-1.5 h-3.5 w-3.5" />
            ) : (
              <Play className="mr-1.5 h-3.5 w-3.5" />
            )}
            {tail ? "Following" : "Paused"}
          </Button>
          {logs.data ? (
            <Badge variant="outline" className="font-mono text-xs">
              {logs.data.entries.length} of {logs.data.capacity}
            </Badge>
          ) : null}
        </div>
      </div>

      <div className="border-border bg-card rounded-md border">
        {logs.isLoading ? (
          <Skeleton className="h-96 w-full" />
        ) : logs.error ? (
          <p className="text-destructive p-4 text-sm">Failed to load logs.</p>
        ) : logs.data && logs.data.entries.length > 0 ? (
          <div className="max-h-[70vh] overflow-y-auto">
            <ol className="divide-border divide-y">
              {/* Newest at the top mirrors a tail's natural reading order. */}
              {[...logs.data.entries].reverse().map((entry) => (
                <LogRow key={entry.id} entry={entry} />
              ))}
            </ol>
          </div>
        ) : (
          <p className="text-muted-foreground p-4 text-sm">
            No log entries match this filter.
          </p>
        )}
      </div>

      <p className="text-muted-foreground text-xs">
        Buffer is in-process and bounded — it loses everything on restart, and
        the oldest entries fall off once it fills. For long-term retention, ship
        structured JSON to Loki.
      </p>
    </div>
  );
}

function LogRow({ entry }: { entry: LogEntryView }) {
  const fieldEntries = Object.entries(entry.fields);
  return (
    <li className="grid grid-cols-[5rem_8rem_1fr] items-baseline gap-3 px-3 py-2 text-xs">
      <span className="text-muted-foreground font-mono">
        {formatHms(entry.timestamp)}
      </span>
      <LevelChip level={entry.level} />
      <div className="min-w-0">
        <p className="text-foreground/95 font-mono">{entry.message}</p>
        <p className="text-muted-foreground mt-0.5 font-mono text-[10px]">
          {entry.target}
        </p>
        {fieldEntries.length > 0 ? (
          <div className="mt-1 flex flex-wrap gap-1">
            {fieldEntries.map(([k, v]) => (
              <span
                key={k}
                className="bg-muted text-muted-foreground rounded px-1.5 py-0.5 font-mono text-[10px]"
              >
                {k}={truncate(v, 64)}
              </span>
            ))}
          </div>
        ) : null}
      </div>
    </li>
  );
}

function LevelChip({ level }: { level: string }) {
  const tone = LEVEL_TONE[level] ?? "border-border text-muted-foreground";
  return (
    <Badge
      variant="outline"
      className={cn("justify-center font-mono text-[10px]", tone)}
    >
      {level}
    </Badge>
  );
}

const LEVEL_TONE: Record<string, string> = {
  error: "border-red-500/40 text-red-400",
  warn: "border-amber-500/40 text-amber-400",
  info: "border-sky-500/40 text-sky-400",
  debug: "border-border text-muted-foreground",
  trace: "border-border text-muted-foreground/70",
};

function formatHms(iso: string): string {
  const d = new Date(iso);
  return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
}

function pad(n: number): string {
  return n < 10 ? `0${n}` : String(n);
}

function truncate(s: string, n: number): string {
  return s.length <= n ? s : `${s.slice(0, n - 1)}…`;
}
