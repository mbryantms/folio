"use client";

import { ChevronLeft, ChevronRight, Pause, Play, X } from "lucide-react";
import Link from "next/link";
import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";

import { NativeSelect } from "@/components/folio/native-select";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import type { DayPages, Run } from "@/lib/log/overview";
import {
  SLIDE_TITLES,
  storySlides,
  type MonthBar,
  type Rhythm,
  type SlideId,
  type TopSeries,
} from "@/lib/log/year";
import { useReducedMotion } from "@/lib/visual-effects";

import { YearSlide } from "./YearSlides";

/** Everything the story shows, derived from the year's stats + log. */
export type YearModel = {
  year: number;
  /** The year is still running ("so far"). */
  inProgress: boolean;
  pages: number;
  issues: number;
  activeMs: number;
  sessions: number;
  daysActive: number;
  /** Days in the year, or elapsed so far while it is running. */
  daysInScope: number;
  months: MonthBar[];
  /** Index into `months`, -1 when there was no reading. */
  busiest: number;
  bigDay: DayPages | null;
  run: Run | null;
  rhythm: Rhythm | null;
  top: (TopSeries & { cover: string | null; finished: number })[];
  mosaic: {
    covers: { key: string; src: string; title: string }[];
    total: number;
    /** The year has more finished issues than were fetched. */
    more: boolean;
  };
  coversLoading: boolean;
};

/** Accent for text: the theme's `--primary` on dark themes, darkened toward
 *  the foreground on light ones so small amber/green text keeps contrast
 *  on a white card (`light-dark` keys off each theme's `color-scheme`). */
const ACCENT_TEXT =
  "light-dark(color-mix(in oklab, var(--primary) 72%, var(--foreground)), var(--primary))";

/** How long a slide stays up while auto-advancing. */
export const SLIDE_MS = 8_000;
/** Horizontal travel that counts as a swipe. */
const SWIPE_PX = 48;

/**
 * "Your <year> in comics": a full-screen story of slides. Tap or click the
 * right / left half, use the arrow keys, swipe, or jump with the progress
 * segments. Auto-advances, pausing on hover, hold, focus inside a slide or
 * the pause button — and never under reduced motion, where slides also
 * appear without motion and numbers show their final values.
 */
export function YearStory({
  year,
  model,
  loading,
  error,
  years,
  onPickYear,
  onClose,
}: {
  year: number;
  model: YearModel | null;
  loading: boolean;
  error: boolean;
  years: number[];
  onPickYear: (year: number) => void;
  onClose: () => void;
}) {
  const reduced = useReducedMotion();
  const slides: SlideId[] = useMemo(
    () =>
      model
        ? storySlides({
            pages: model.pages,
            sessions: model.sessions,
            daysActive: model.daysActive,
            busiest: model.busiest,
            topCount: model.top.length,
            hasRhythm: !!model.rhythm,
            run: model.run,
            // Keep the mosaic slot while covers load so slides don't shift.
            finished: model.coversLoading ? 1 : model.mosaic.total,
          })
        : [],
    [model],
  );
  const count = slides.length;
  const [index, setIndex] = useState(0);
  const at = Math.min(index, Math.max(0, count - 1));
  const slide = slides[at];

  const go = useCallback(
    (delta: number) =>
      setIndex((i) => Math.max(0, Math.min(count - 1, i + delta))),
    [count],
  );
  const goRef = useRef(go);
  useEffect(() => {
    goRef.current = go;
  }, [go]);

  // ---- pausing ----
  const [userPaused, setUserPaused] = useState(false);
  const [hover, setHover] = useState(false);
  const [hold, setHold] = useState(false);
  const [focusIn, setFocusIn] = useState(false);
  const [hidden, setHidden] = useState(false);
  const autoplay = !reduced && count > 1 && at < count - 1;
  const paused = userPaused || hover || hold || focusIn || hidden;

  useEffect(() => {
    const onVis = () => setHidden(document.visibilityState === "hidden");
    document.addEventListener("visibilitychange", onVis);
    return () => document.removeEventListener("visibilitychange", onVis);
  }, []);

  // ---- progress (Web Animations; its end advances the story) ----
  const fills = useRef<(HTMLSpanElement | null)[]>([]);
  const anim = useRef<Animation | null>(null);
  useEffect(() => {
    const el = fills.current[at];
    if (!autoplay || !el || typeof el.animate !== "function") return;
    const a = el.animate(
      [{ transform: "scaleX(0)" }, { transform: "scaleX(1)" }],
      { duration: SLIDE_MS, easing: "linear", fill: "forwards" },
    );
    a.onfinish = () => goRef.current(1);
    anim.current = a;
    return () => {
      a.onfinish = null;
      a.cancel();
      anim.current = null;
    };
  }, [at, autoplay, slide]);
  useEffect(() => {
    const a = anim.current;
    if (!a) return;
    if (paused) a.pause();
    else if (a.playState === "paused") a.play();
  }, [paused, at, autoplay]);

  // ---- focus ----
  const stageRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    stageRef.current?.focus({ preventScroll: true });
  }, []);
  // A slide change can unmount the focused control (Replay on the last
  // slide); hand focus back to the stage rather than the page body.
  useLayoutEffect(() => {
    const active = document.activeElement;
    if (!active || active === document.body)
      stageRef.current?.focus({ preventScroll: true });
  }, [at]);

  // ---- keys ----
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.defaultPrevented || e.altKey || e.ctrlKey || e.metaKey) return;
      // Arrow keys belong to the year picker (and any field) while focused.
      const t = e.target;
      if (
        t instanceof Element &&
        t.closest("select, input, textarea, [contenteditable='true']")
      )
        return;
      if (e.key === "ArrowRight") {
        e.preventDefault();
        goRef.current(1);
      } else if (e.key === "ArrowLeft") {
        e.preventDefault();
        goRef.current(-1);
      } else if (e.key === "Escape") {
        e.preventDefault();
        onClose();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  // ---- pointer: hover / hold pause, swipe ----
  const swiped = useRef(false);
  useEffect(() => {
    const el = stageRef.current;
    if (!el) return;
    let start: { x: number; y: number } | null = null;
    const down = (e: PointerEvent) => {
      if (!e.isPrimary) return;
      start = { x: e.clientX, y: e.clientY };
      swiped.current = false;
      setHold(true);
    };
    const up = (e: PointerEvent) => {
      setHold(false);
      if (!start) return;
      const dx = e.clientX - start.x;
      const dy = e.clientY - start.y;
      start = null;
      if (Math.abs(dx) >= SWIPE_PX && Math.abs(dx) > Math.abs(dy) * 1.5) {
        swiped.current = true;
        goRef.current(dx < 0 ? 1 : -1);
      }
    };
    const cancel = () => {
      start = null;
      setHold(false);
    };
    const enter = (e: PointerEvent) => {
      if (e.pointerType === "mouse") setHover(true);
    };
    const leave = (e: PointerEvent) => {
      if (e.pointerType === "mouse") setHover(false);
      cancel();
    };
    el.addEventListener("pointerdown", down);
    el.addEventListener("pointerup", up);
    el.addEventListener("pointercancel", cancel);
    el.addEventListener("pointerenter", enter);
    el.addEventListener("pointerleave", leave);
    return () => {
      el.removeEventListener("pointerdown", down);
      el.removeEventListener("pointerup", up);
      el.removeEventListener("pointercancel", cancel);
      el.removeEventListener("pointerenter", enter);
      el.removeEventListener("pointerleave", leave);
    };
  }, []);
  const tap = (delta: number) => {
    if (swiped.current) {
      swiped.current = false;
      return;
    }
    go(delta);
  };

  const live = autoplay && !paused ? "off" : "polite";

  return (
    <div
      className="bg-background text-foreground fixed inset-0 z-50 flex flex-col overflow-hidden"
      style={{ "--year-accent": ACCENT_TEXT } as React.CSSProperties}
    >
      {/* Accent glow behind the stage, from the theme's own tokens. */}
      <div
        aria-hidden
        className="pointer-events-none absolute inset-0 bg-[radial-gradient(60%_50%_at_50%_0%,color-mix(in_oklab,var(--primary)_22%,transparent),transparent),radial-gradient(50%_40%_at_100%_100%,color-mix(in_oklab,var(--primary)_12%,transparent),transparent)]"
      />
      <div className="relative mx-auto flex h-full w-full max-w-xl flex-col px-3 pt-[max(0.75rem,var(--safe-top))] pb-[max(0.75rem,var(--safe-bottom))] sm:px-4 sm:py-6">
        <header className="space-y-2">
          {count > 0 ? (
            <ol className="flex gap-1" aria-label="Slides">
              {slides.map((id, i) => (
                <li key={id} className="flex-1">
                  <button
                    type="button"
                    onClick={() => setIndex(i)}
                    aria-label={`Slide ${i + 1}: ${SLIDE_TITLES[id]}`}
                    aria-current={i === at ? "step" : undefined}
                    className="focus-visible:ring-ring group block w-full rounded-full py-2 focus-visible:ring-2 focus-visible:outline-none"
                  >
                    <span
                      className={
                        i === at
                          ? "bg-foreground/30 block h-1 overflow-hidden rounded-full"
                          : "bg-foreground/15 block h-1 overflow-hidden rounded-full"
                      }
                    >
                      <span
                        ref={(el) => {
                          fills.current[i] = el;
                        }}
                        className="bg-primary block h-full origin-left rounded-full"
                        style={{
                          transform:
                            i < at || (i === at && !autoplay)
                              ? "scaleX(1)"
                              : "scaleX(0)",
                        }}
                      />
                    </span>
                  </button>
                </li>
              ))}
            </ol>
          ) : null}
          <div className="flex items-center gap-2">
            <h1 className="min-w-0 flex-1 truncate text-sm font-semibold">
              Your {year} in comics
            </h1>
            {years.length > 1 ? (
              <label className="flex items-center gap-1.5">
                <span className="sr-only">Year</span>
                <NativeSelect
                  size="sm"
                  value={String(year)}
                  onChange={(v) => onPickYear(+v)}
                  options={years.map((y) => ({
                    value: String(y),
                    label: String(y),
                  }))}
                />
              </label>
            ) : null}
            <Button variant="ghost" size="icon" asChild>
              <Link href="/log" aria-label="Close year in review">
                <X aria-hidden />
              </Link>
            </Button>
          </div>
        </header>

        <div
          ref={stageRef}
          role="region"
          aria-label={`Your ${year} in comics`}
          aria-roledescription="story"
          tabIndex={-1}
          onFocus={(e) => {
            if (e.target !== stageRef.current) setFocusIn(true);
          }}
          onBlur={() => setFocusIn(false)}
          className="bg-card relative mt-2 min-h-0 flex-1 touch-pan-y overflow-hidden rounded-3xl border shadow-[0_30px_80px_-40px_color-mix(in_oklab,var(--primary)_60%,transparent)] outline-none select-none"
        >
          {count > 1 ? (
            <>
              <button
                type="button"
                tabIndex={-1}
                aria-label="Previous slide"
                disabled={at === 0}
                onClick={() => tap(-1)}
                className="absolute inset-y-0 left-0 z-0 w-1/2 cursor-w-resize disabled:cursor-default"
              />
              <button
                type="button"
                tabIndex={-1}
                aria-label="Next slide"
                disabled={at === count - 1}
                onClick={() => tap(1)}
                className="absolute inset-y-0 right-0 z-0 w-1/2 cursor-e-resize disabled:cursor-default"
              />
            </>
          ) : null}
          <div
            aria-live={live}
            className="pointer-events-none relative z-10 h-full"
          >
            {loading || (!model && !error) ? (
              <StorySkeleton />
            ) : error || !model || !slide ? (
              <div className="flex h-full flex-col items-center justify-center gap-4 p-6 text-center">
                <p className="text-sm">
                  Couldn&apos;t load your {year} in review.
                </p>
                <Button
                  variant="outline"
                  asChild
                  className="pointer-events-auto"
                >
                  <Link href="/log">Back to your log</Link>
                </Button>
              </div>
            ) : (
              <section
                key={`${year}-${slide}`}
                aria-roledescription="slide"
                aria-label={`${at + 1} of ${count}: ${SLIDE_TITLES[slide]}`}
                className="h-full"
              >
                <YearSlide
                  id={slide}
                  model={model}
                  reduced={reduced}
                  years={years}
                  onReplay={() => setIndex(0)}
                  onPickYear={onPickYear}
                />
              </section>
            )}
          </div>
        </div>

        {count > 1 ? (
          <footer className="mt-2 flex items-center justify-between gap-2">
            <Button
              variant="ghost"
              size="icon"
              onClick={() => go(-1)}
              disabled={at === 0}
              aria-label="Previous slide"
            >
              <ChevronLeft aria-hidden />
            </Button>
            <div className="flex items-center gap-1">
              <p className="text-muted-foreground text-xs tabular-nums">
                {at + 1} / {count}
              </p>
              {!reduced ? (
                <Button
                  variant="ghost"
                  size="icon-sm"
                  onClick={() => setUserPaused((p) => !p)}
                  aria-label={userPaused ? "Play story" : "Pause story"}
                  aria-pressed={userPaused}
                  disabled={!autoplay}
                >
                  {userPaused ? <Play aria-hidden /> : <Pause aria-hidden />}
                </Button>
              ) : null}
            </div>
            <Button
              variant="ghost"
              size="icon"
              onClick={() => go(1)}
              disabled={at === count - 1}
              aria-label="Next slide"
            >
              <ChevronRight aria-hidden />
            </Button>
          </footer>
        ) : null}
      </div>
    </div>
  );
}

function StorySkeleton() {
  return (
    <div
      className="flex h-full flex-col items-center justify-center gap-4 p-8"
      aria-busy="true"
    >
      <Skeleton className="h-4 w-28" />
      <Skeleton className="h-24 w-56 rounded-2xl" />
      <Skeleton className="h-4 w-40" />
      <span className="sr-only">Loading your year in review…</span>
    </div>
  );
}
