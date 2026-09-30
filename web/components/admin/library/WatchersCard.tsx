"use client";

import Link from "next/link";
import { Eye, Loader2 } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { useWatchers } from "@/lib/api/queries";
import type { LibraryWatcherView, WatchMode } from "@/lib/api/types";
import { timeAgo } from "@/lib/sessions";
import { statusTone, type StatusTone } from "@/lib/ui/status-tone";
import { cn } from "@/lib/utils";

const MODE_TONE: Record<WatchMode, StatusTone> = {
  inotify: "success",
  poll: "info",
  disabled: "neutral",
};

/** Human description of one library's last watcher trigger. Exported for
 *  unit tests. */
export function describeLastTrigger(
  w: LibraryWatcherView["watcher"],
  now: number = Date.now(),
): string {
  if (!w.last_trigger_at) {
    return w.mode === "disabled" ? "—" : "No changes seen yet";
  }
  const when = timeAgo(w.last_trigger_at, now);
  const what =
    w.last_trigger_dirs === 0
      ? "full rescan (event overflow)"
      : `${w.last_trigger_dirs} folder${w.last_trigger_dirs === 1 ? "" : "s"}`;
  return `${when} · ${what}${w.last_trigger_coalesced ? " · joined running scan" : ""}`;
}

/** Scan-dashboard card (WP-3.1): per-library file-watcher mode
 *  (inotify / poll / disabled) and the last trigger. */
export function WatchersCard() {
  const { data, isLoading, error } = useWatchers();

  return (
    <Card>
      <CardHeader className="flex flex-row items-center justify-between pb-2">
        <CardTitle className="flex items-center gap-2 text-sm font-medium">
          <Eye className="h-4 w-4" /> File watchers
        </CardTitle>
        {data && (
          <span className="text-muted-foreground text-xs">
            debounce {data.debounce_secs}s · network poll{" "}
            {data.poll_interval_secs}s{data.force_poll ? " · poll forced" : ""}
          </span>
        )}
      </CardHeader>
      <CardContent>
        {isLoading ? (
          <div className="text-muted-foreground flex items-center gap-2 text-sm">
            <Loader2 className="h-4 w-4 animate-spin" /> Loading watchers…
          </div>
        ) : error || !data ? (
          <p className="text-destructive text-sm">
            Failed to load watcher status.
          </p>
        ) : data.libraries.length === 0 ? (
          <p className="text-muted-foreground text-sm">No libraries yet.</p>
        ) : (
          <ul className="divide-border divide-y">
            {data.libraries.map((l) => (
              <li
                key={l.library_id}
                className="flex flex-col gap-1 py-2 sm:flex-row sm:items-center sm:justify-between sm:gap-3"
              >
                <span className="flex min-w-0 items-center gap-2">
                  <Badge
                    variant="outline"
                    className={cn(
                      "shrink-0 font-mono text-[11px]",
                      statusTone(MODE_TONE[l.watcher.mode]),
                    )}
                  >
                    {l.watcher.mode}
                  </Badge>
                  <Link
                    href={`/admin/libraries/${l.library_slug}`}
                    className="truncate text-sm hover:underline"
                  >
                    {l.library_name}
                  </Link>
                  {l.watcher.filesystem && (
                    <span className="text-muted-foreground shrink-0 text-xs">
                      {l.watcher.filesystem}
                    </span>
                  )}
                </span>
                <span className="text-muted-foreground min-w-0 text-xs sm:text-right">
                  {describeLastTrigger(l.watcher)}
                  {l.watcher.detail && (
                    <span className="block truncate" title={l.watcher.detail}>
                      {l.watcher.detail}
                    </span>
                  )}
                </span>
              </li>
            ))}
          </ul>
        )}
      </CardContent>
    </Card>
  );
}
