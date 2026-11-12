"use client";

import { AlertTriangle, RefreshCw } from "lucide-react";

import { Button } from "@/components/ui/button";
import type { CblListView } from "@/lib/api/types";
import {
  failureCountLabel,
  refreshFailureAdvice,
  refreshFailureReason,
} from "@/lib/cbl/refresh-failure";
import { cn } from "@/lib/utils";

/**
 * A reading list whose refreshes are failing, in words: what is wrong,
 * since when, the last attempt and how many have failed, and what the
 * reader can do. Renders nothing for a healthy list. `onRefresh` (given
 * only to someone who may refresh the list) adds a "Refresh now" button;
 * a failed retry is reported by the mutation hook's own error toast.
 * `canRefresh` (default: whether `onRefresh` is given) picks the advice,
 * for a place that has its own refresh button.
 */
export function RefreshFailureNotice({
  list,
  onRefresh,
  refreshing = false,
  canRefresh = onRefresh !== undefined,
  className,
}: {
  list: Pick<
    CblListView,
    "refresh_failure" | "last_refresh_attempt_at" | "last_refreshed_at"
  >;
  onRefresh?: () => void;
  refreshing?: boolean;
  canRefresh?: boolean;
  className?: string;
}) {
  const failure = list.refresh_failure;
  if (!failure) return null;
  const since = new Date(failure.failing_since).toLocaleString();
  const attempt = list.last_refresh_attempt_at
    ? new Date(list.last_refresh_attempt_at).toLocaleString()
    : null;
  const reason = refreshFailureReason(failure);
  return (
    <div
      role="status"
      data-refresh-failure=""
      className={cn(
        "border-warning/40 bg-warning/10 flex flex-wrap items-start gap-3 rounded-md border p-3 text-sm",
        className,
      )}
    >
      <AlertTriangle
        className="text-warning mt-0.5 size-4 shrink-0"
        aria-hidden="true"
      />
      <div className="min-w-0 flex-1 basis-56 space-y-1">
        <p className="text-foreground font-medium">
          Refresh failing: {reason}.
        </p>
        <p className="text-muted-foreground">
          {`Failing since ${since}${attempt ? `; last attempt ${attempt}` : ""} (${failureCountLabel(failure.consecutive_failures)}). `}
          {list.last_refreshed_at
            ? `The list still shows its copy from ${new Date(list.last_refreshed_at).toLocaleDateString()}.`
            : "The list still shows the copy it was imported with."}
        </p>
        <p className="text-muted-foreground">
          {refreshFailureAdvice(failure.kind, canRefresh)}
        </p>
      </div>
      {onRefresh ? (
        <Button
          type="button"
          size="sm"
          variant="outline"
          className="shrink-0 whitespace-nowrap"
          onClick={onRefresh}
          disabled={refreshing}
        >
          <RefreshCw
            className={cn("mr-1 size-3", refreshing && "animate-spin")}
            aria-hidden="true"
          />
          Refresh now
        </Button>
      ) : null}
    </div>
  );
}
