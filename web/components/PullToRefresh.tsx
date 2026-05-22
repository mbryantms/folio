"use client";

import { ArrowDown, Loader2 } from "lucide-react";
import { useRouter } from "next/navigation";
import { useCallback } from "react";

import { useQueryClient } from "@tanstack/react-query";

import { cn } from "@/lib/utils";
import {
  usePullToRefresh,
  type PullState,
} from "@/lib/use-pull-to-refresh";

/**
 * Visual wrapper around the `usePullToRefresh` hook. Renders a
 * floating indicator at the top of the viewport that grows as the
 * user drags down and locks in once the pull crosses the threshold.
 *
 * The wrapper mounts in the app shells (`MainShell` and `AdminShell`)
 * so it covers every library / admin / settings route. The reader
 * route owns its own touch surface and intentionally does not mount
 * it.
 *
 * The refresh action invalidates the active TanStack Query cache and
 * calls `router.refresh()` so React Server Components for the
 * current page re-render against the freshly-fetched data. Both calls
 * are quick and idempotent.
 */
export function PullToRefresh({ enabled = true }: { enabled?: boolean }) {
  const router = useRouter();
  const queryClient = useQueryClient();

  const refresh = useCallback(async () => {
    await queryClient.invalidateQueries();
    router.refresh();
  }, [queryClient, router]);

  const { state, distance } = usePullToRefresh({ onRefresh: refresh, enabled });

  if (state === "idle") return null;

  return (
    <PullToRefreshIndicator state={state} distance={distance} />
  );
}

function PullToRefreshIndicator({
  state,
  distance,
}: {
  state: PullState;
  distance: number;
}) {
  const isRefreshing = state === "refreshing";
  const isArmed = state === "armed";
  // The indicator moves with the pull, but capped so it doesn't
  // disappear off the bottom of the screen on very deep pulls. The
  // `state === "refreshing"` branch pins it to a fixed offset while
  // the spinner is visible.
  const offsetPx = isRefreshing ? 56 : Math.min(distance, 140);
  const rotation = isArmed ? 180 : Math.min(distance / 80, 1) * 180;

  return (
    <div
      // role="status" + aria-live so screen readers announce the refresh
      // without it being a focus stop.
      role="status"
      aria-live="polite"
      className="pointer-events-none fixed inset-x-0 top-0 z-50 flex justify-center"
      style={{
        transform: `translateY(${offsetPx - 56}px)`,
        transition: state === "pulling" ? "none" : "transform 200ms ease-out",
      }}
    >
      <div
        className={cn(
          "bg-background/95 border-border ring-border/40 mt-2 flex h-12 w-12 items-center justify-center rounded-full border shadow-sm ring-1 backdrop-blur",
          isArmed && "border-primary text-primary",
        )}
      >
        {isRefreshing ? (
          <Loader2 className="h-5 w-5 animate-spin" aria-hidden="true" />
        ) : (
          <ArrowDown
            className="h-5 w-5"
            style={{ transform: `rotate(${rotation}deg)` }}
            aria-hidden="true"
          />
        )}
        <span className="sr-only">
          {isRefreshing
            ? "Refreshing"
            : isArmed
              ? "Release to refresh"
              : "Pull down to refresh"}
        </span>
      </div>
    </div>
  );
}
