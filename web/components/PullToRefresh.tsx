"use client";

import { useRouter } from "next/navigation";
import { useCallback, useEffect } from "react";

import { useQueryClient, type QueryClient } from "@tanstack/react-query";

import { queryKeys } from "@/lib/api/query-keys";
import type { ContinueReadingView, OnDeckView } from "@/lib/api/types";
import { cn } from "@/lib/utils";
import {
  computePullLayout,
  usePullToRefresh,
  type PullState,
} from "@/lib/use-pull-to-refresh";
import { useReducedMotion } from "@/lib/visual-effects";

/**
 * Branded pull-to-refresh. Mounted in the app shells (`MainShell` and
 * `AdminShell`) so it covers every library / admin / settings route;
 * the reader owns its own touch surface and does not mount it.
 *
 * As the user pulls, the page content (`#main-content`) translates
 * down and a small fan of three comic covers appears in the gap it
 * opens, beneath the sticky top bar. Past the threshold the fan is
 * fully spread; on release the content holds the gap open while the
 * covers bob, then springs back once the refresh settles.
 *
 * The refresh action invalidates the active TanStack Query cache and
 * calls `router.refresh()` so React Server Components for the current
 * page re-render against the freshly-fetched data.
 */

const THRESHOLD = 80;
/** Cover size: 44x66 (2:3). The indicator box adds room for the fan
 *  and the bob so the clipped gap doesn't shave the covers. */
const COVER_W = 44;
const COVER_H = 66;
const INDICATOR_W = 112;
const INDICATOR_H = 76;

/** Release / settle motion for the content and the gap. The spring
 *  curve overshoots slightly; reduced motion gets a plain ease-out. */
const SPRING = "420ms cubic-bezier(0.34, 1.36, 0.64, 1)";
const PLAIN = "180ms ease-out";
/** Clear the inline transform once the spring-back has finished, so an
 *  idle page carries no transform (a transformed ancestor would become
 *  the containing block for `position: fixed` descendants). */
const CLEAR_AFTER_MS = 460;
/** Minimum time the indicator stays open after a release. */
const MIN_REFRESH_MS = 700;

export function PullToRefresh({
  enabled = true,
  contentId = "main-content",
}: {
  enabled?: boolean;
  /** Id of the element that slides down to make room for the indicator. */
  contentId?: string;
}) {
  const router = useRouter();
  const queryClient = useQueryClient();
  const reduced = useReducedMotion();

  const refresh = useCallback(async () => {
    // Hold the open state for a beat even when the refetch is instant,
    // so the bob reads as "refreshed" rather than a flicker.
    await Promise.all([
      queryClient.invalidateQueries().then(() => router.refresh()),
      new Promise((r) => window.setTimeout(r, MIN_REFRESH_MS)),
    ]);
  }, [queryClient, router]);

  const { state, distance } = usePullToRefresh({
    onRefresh: refresh,
    enabled,
    threshold: THRESHOLD,
  });

  const layout = computePullLayout(distance, THRESHOLD, INDICATOR_H);
  const tracking = state === "pulling" || state === "armed";
  const transition = tracking ? "none" : reduced ? PLAIN : SPRING;
  const offset = layout.contentOffset;

  // Drive the content's transform imperatively: it belongs to the
  // shell's `SidebarInset`, which React renders without an inline
  // transform, so setting it here never fights reconciliation.
  useEffect(() => {
    const el = document.getElementById(contentId);
    if (!el) return;
    if (offset === 0) {
      if (!el.style.transform) return;
      el.style.transition =
        transition === "none" ? "none" : `transform ${transition}`;
      el.style.transform = "translateY(0px)";
      const t = window.setTimeout(() => clearInline(el), CLEAR_AFTER_MS);
      return () => window.clearTimeout(t);
    }
    el.style.willChange = "transform";
    el.style.transition =
      transition === "none" ? "none" : `transform ${transition}`;
    el.style.transform = `translateY(${offset}px)`;
  }, [contentId, offset, transition]);

  // Leave nothing behind if the shell unmounts mid-pull.
  useEffect(() => {
    return () => {
      const el = document.getElementById(contentId);
      if (el) clearInline(el);
    };
  }, [contentId]);

  // Covers come from whatever rail data is already cached (no fetch of
  // our own); tinted blanks stand in when nothing is.
  const covers = state === "idle" ? [] : cachedCovers(queryClient);

  return (
    <PullToRefreshIndicator
      state={state}
      gap={layout.gap}
      indicatorY={layout.indicatorY}
      progress={layout.progress}
      covers={covers}
      reduced={reduced}
      transition={transition}
    />
  );
}

function clearInline(el: HTMLElement) {
  el.style.transition = "";
  el.style.transform = "";
  el.style.willChange = "";
}

/** Up to three distinct cover URLs from the cached Continue Reading /
 *  On Deck rails. Reads the cache only; never triggers a fetch. */
export function cachedCovers(queryClient: QueryClient): string[] {
  const out: string[] = [];
  const add = (url: string | null | undefined) => {
    if (url && out.length < 3 && !out.includes(url)) out.push(url);
  };
  queryClient
    .getQueryData<ContinueReadingView>(queryKeys.continueReading)
    ?.items.forEach((c) => add(c.issue.cover_url));
  queryClient
    .getQueryData<OnDeckView>(queryKeys.onDeck)
    ?.items.forEach((c) => add(c.issue.cover_url));
  return out;
}

/** Blank-cover tints, left to right (the middle one sits on top). Mixed
 *  with the page background so they stay opaque where they overlap. */
const BLANK_TINTS = [40, 80, 58].map(
  (pct) => `color-mix(in oklab, var(--primary) ${pct}%, var(--background))`,
);

function PullToRefreshIndicator({
  state,
  gap,
  indicatorY,
  progress,
  covers,
  reduced,
  transition,
}: {
  state: PullState;
  gap: number;
  indicatorY: number;
  progress: number;
  covers: string[];
  reduced: boolean;
  transition: string;
}) {
  const isRefreshing = state === "refreshing";
  const isArmed = state === "armed";
  // Fan: the outer covers slide out and tilt as the pull progresses.
  // Reduced motion shows a static fan that simply fades in.
  const k = reduced ? 1 : progress;
  const scale = reduced ? 1 : 0.7 + 0.3 * progress;
  const opacity = Math.min(1, gap / 32);
  const timed = (prop: string) =>
    transition === "none" ? "none" : `${prop} ${transition}`;

  return (
    <div
      // role="status" + aria-live so screen readers announce the refresh
      // without it being a focus stop.
      role="status"
      aria-live="polite"
      data-pull-state={state}
      // Clipped to the gap the content opened: below the sticky top bar
      // (z-30) and never over the page.
      className="pointer-events-none fixed inset-x-0 top-(--topbar-h) z-20 overflow-hidden"
      style={{ height: gap, transition: timed("height") }}
    >
      <style>{PTR_KEYFRAMES}</style>
      <div
        className="absolute top-0 left-1/2"
        style={{
          width: INDICATOR_W,
          height: INDICATOR_H,
          marginLeft: -INDICATOR_W / 2,
          transform: `translateY(${indicatorY}px) scale(${scale})`,
          transformOrigin: "50% 100%",
          opacity,
          transition:
            transition === "none"
              ? "none"
              : `transform ${transition}, opacity ${transition}`,
        }}
      >
        {[0, 1, 2].map((i) => {
          const url = covers[i];
          const s = i - 1; // -1, 0, 1
          return (
            <span
              key={i}
              className="absolute bottom-1 left-1/2"
              style={{
                width: COVER_W,
                height: COVER_H,
                marginLeft: -COVER_W / 2,
                transformOrigin: "50% 100%",
                transform: `translateX(${s * 24 * k}px) rotate(${s * 14 * k}deg)`,
                transition: reduced ? "none" : "transform 150ms ease-out",
                zIndex: i === 1 ? 2 : 1,
              }}
            >
              <span
                className={cn(
                  "border-border/60 block h-full w-full overflow-hidden rounded-[4px] border shadow-[0_3px_10px_rgb(0_0_0/0.35)]",
                  isArmed && "ring-primary/70 ring-1",
                )}
                style={{
                  background: url ? undefined : BLANK_TINTS[i],
                  ...(isRefreshing && !reduced
                    ? {
                        animation:
                          "folio-ptr-bob 600ms ease-in-out infinite alternate",
                        animationDelay: `${i * 120}ms`,
                      }
                    : null),
                }}
              >
                {url ? (
                  // eslint-disable-next-line @next/next/no-img-element
                  <img
                    src={url}
                    alt=""
                    draggable={false}
                    className="h-full w-full object-cover"
                  />
                ) : null}
              </span>
            </span>
          );
        })}
      </div>
      <span className="sr-only">
        {isRefreshing
          ? "Refreshing"
          : isArmed
            ? "Release to refresh"
            : state === "pulling"
              ? "Pull down to refresh"
              : ""}
      </span>
    </div>
  );
}

const PTR_KEYFRAMES =
  "@keyframes folio-ptr-bob{from{transform:translateY(0)}to{transform:translateY(-7px)}}";
