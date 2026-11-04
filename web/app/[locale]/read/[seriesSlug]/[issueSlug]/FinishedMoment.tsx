"use client";

import { Check } from "lucide-react";
import { useEffect, useState } from "react";

import type { SeriesFinishProgress } from "@/lib/reader/finish-progress";
import { cn } from "@/lib/utils";
import { useReducedMotion } from "@/lib/visual-effects";

const RING_R = 24;
const RING_C = 2 * Math.PI * RING_R;
/** Wait for the card's slide-in before the ring moves. */
const RING_DELAY_MS = 420;
const RING_MS = 1300;

/**
 * "Finishing moment" (folio-flair-1.0, a rework of the end-of-issue card
 * header). Names the issue just finished and shows the series' progress
 * as a ring that fills from before-this-issue to now, counting the
 * percentage up alongside. Finishing the last unread issue turns the ring
 * green with a check: "Series complete". Reduced motion shows the end
 * state at once.
 */
export function FinishedMoment({
  open,
  issueLabel,
  progress,
}: {
  open: boolean;
  issueLabel: string;
  progress: SeriesFinishProgress | null;
}) {
  const reduced = useReducedMotion();
  const total = progress?.total ?? 0;
  const before = progress?.before ?? 0;
  const after = progress?.after ?? 0;
  const gained = after - before;
  const complete = !!progress && after >= total && gained > 0;

  // `shown` is the count the ring and figure display; it animates from
  // `before` to `after` each time the card opens.
  const [shown, setShown] = useState(before);
  const [lastOpen, setLastOpen] = useState(open);
  if (open !== lastOpen) {
    setLastOpen(open);
    if (open) setShown(reduced ? after : before);
  }
  useEffect(() => {
    if (!open || reduced || gained <= 0) return;
    let raf = 0;
    const start = window.setTimeout(() => {
      const t0 = performance.now();
      const tick = (t: number) => {
        const k = Math.min(1, (t - t0) / RING_MS);
        const eased = 1 - Math.pow(1 - k, 3);
        setShown(before + gained * eased);
        if (k < 1) raf = requestAnimationFrame(tick);
      };
      raf = requestAnimationFrame(tick);
    }, RING_DELAY_MS);
    return () => {
      window.clearTimeout(start);
      cancelAnimationFrame(raf);
    };
  }, [open, reduced, before, gained]);

  const value = gained > 0 ? shown : after;
  const frac = total > 0 ? value / total : 0;
  const pct = Math.round(frac * 100);
  const done = complete && value >= after - 0.001;

  return (
    <div className="space-y-3">
      <div className="min-w-0">
        <p
          className={cn(
            "text-[11px] font-bold tracking-[0.1em] uppercase",
            done ? "text-success" : "text-primary",
          )}
        >
          {done ? "Series complete" : "Finished"}
        </p>
        <h2
          className="mt-0.5 truncate text-base leading-snug font-semibold"
          title={issueLabel}
        >
          {issueLabel}
        </h2>
      </div>
      {progress ? (
        <div className="flex items-center gap-3">
          <div className="relative size-14 shrink-0">
            <svg
              viewBox="0 0 56 56"
              aria-hidden
              className={cn(
                "size-14 -rotate-90",
                done && !reduced && "finish-ring-pop",
              )}
            >
              <circle
                cx="28"
                cy="28"
                r={RING_R}
                fill="none"
                strokeWidth="6"
                className="stroke-muted"
              />
              <circle
                cx="28"
                cy="28"
                r={RING_R}
                fill="none"
                strokeWidth="6"
                strokeLinecap="round"
                strokeDasharray={RING_C}
                strokeDashoffset={RING_C * (1 - frac)}
                className={cn(
                  "transition-[stroke] duration-500",
                  done ? "stroke-success" : "stroke-primary",
                )}
              />
            </svg>
            {done ? (
              <Check
                aria-hidden
                className="text-success absolute inset-0 m-auto size-5"
                strokeWidth={3}
              />
            ) : null}
          </div>
          <div className="min-w-0 text-sm">
            <p className="leading-tight">
              <span className="text-lg font-semibold tabular-nums">{pct}%</span>{" "}
              <span className="text-muted-foreground">
                of {progress.seriesName}
              </span>
            </p>
            <p className="text-muted-foreground text-xs tabular-nums">
              {after} of {total} {total === 1 ? "issue" : "issues"} read
              {gained > 0 && !done ? (
                <span className="text-success font-medium">
                  {" "}
                  · +1 this issue
                </span>
              ) : null}
            </p>
          </div>
        </div>
      ) : null}
    </div>
  );
}
