"use client";

/**
 * Linked reading lists: which chapter the reader is in, for the chapter
 * rail's highlight. Measured from the rendered chapter sections against
 * the pinned chapter header's line, the same line the `]` / `[` shortcuts
 * step from.
 */
import * as React from "react";

import { currentChapter } from "@/lib/cbl/linked-plan";

/** Each chapter section's top edge relative to the reading line (the
 *  bottom of the pinned chapter header), in chapter order. A section not
 *  rendered yet (its page not loaded) is `+Infinity`. */
export function chapterTops(count: number): number[] {
  const header = document.querySelector<HTMLElement>(
    '[data-testid="chapter-header"]',
  );
  const line = header
    ? (Number.parseFloat(getComputedStyle(header).top) || 0) +
      header.offsetHeight
    : 0;
  return Array.from({ length: count }, (_, i) => {
    const el = document.querySelector(`[data-chapter-index="${i}"]`);
    return el
      ? el.getBoundingClientRect().top - line
      : Number.POSITIVE_INFINITY;
  });
}

/** `smooth` unless the reader asked the system for less motion. */
export function scrollBehavior(): ScrollBehavior {
  return typeof window !== "undefined" &&
    window.matchMedia?.("(prefers-reduced-motion: reduce)").matches
    ? "auto"
    : "smooth";
}

/** How long the page must stop scrolling before a jump counts as done. */
const SETTLE_MS = 160;
/** A jump that never scrolls (already there, or its page never loads)
 *  lets go of the highlight after this. */
const MAX_HOLD_MS = 3000;

/**
 * The chapter in view, updated as the page scrolls (once per frame).
 *
 * While a jump is under way (`jumpTarget` set, then the smooth scroll it
 * starts), the target is reported instead, so the highlight moves once to
 * where the reader is going rather than ticking through every chapter on
 * the way; measuring resumes once the page stops scrolling.
 */
export function useCurrentChapter(
  count: number,
  jumpTarget: number | null,
): number {
  const [measured, setMeasured] = React.useState(0);
  const [held, setHeld] = React.useState<number | null>(null);
  // Adopt a new jump target during render (not in an effect), so the
  // highlight moves in the same commit as the click.
  const [lastTarget, setLastTarget] = React.useState<number | null>(null);
  if (jumpTarget !== lastTarget) {
    setLastTarget(jumpTarget);
    if (jumpTarget != null) setHeld(jumpTarget);
  }

  React.useEffect(() => {
    if (count < 2) return;
    let frame = 0;
    const measure = () => {
      frame = 0;
      setMeasured(currentChapter(chapterTops(count)));
    };
    const onScroll = () => {
      if (!frame) frame = requestAnimationFrame(measure);
    };
    measure();
    window.addEventListener("scroll", onScroll, { passive: true });
    window.addEventListener("resize", onScroll);
    // A fold toggle or a page of entries landing moves the sections
    // without a scroll event; the page's height changing says so.
    const ro =
      typeof ResizeObserver === "undefined"
        ? null
        : new ResizeObserver(onScroll);
    ro?.observe(document.body);
    return () => {
      if (frame) cancelAnimationFrame(frame);
      window.removeEventListener("scroll", onScroll);
      window.removeEventListener("resize", onScroll);
      ro?.disconnect();
    };
  }, [count]);

  // Let go of a held jump once the page settles after it (or never moved).
  React.useEffect(() => {
    if (held == null || jumpTarget != null) return;
    let settle: ReturnType<typeof setTimeout> | undefined;
    const release = () => {
      setHeld(null);
      setMeasured(currentChapter(chapterTops(count)));
    };
    const onScroll = () => {
      clearTimeout(settle);
      settle = setTimeout(release, SETTLE_MS);
    };
    // No scroll at all: the target was already in place.
    settle = setTimeout(release, SETTLE_MS * 2);
    const cap = setTimeout(release, MAX_HOLD_MS);
    window.addEventListener("scroll", onScroll, { passive: true });
    return () => {
      clearTimeout(settle);
      clearTimeout(cap);
      window.removeEventListener("scroll", onScroll);
    };
  }, [held, jumpTarget, count]);

  return held ?? measured;
}
