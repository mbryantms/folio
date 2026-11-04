"use client";

import { forwardRef, useCallback, useEffect, useRef } from "react";

import { cn } from "@/lib/utils";
import { useVisualEffect } from "@/lib/visual-effects";

/** Largest lean, in degrees, when the pointer sits on a cover's edge. */
const MAX_TILT_DEG = 7;

/**
 * The positioned box that owns a card's cover (`<div className="relative">`
 * in SeriesCard / IssueCard / OnDeckCard / ProgressIssueCard).
 *
 * With the "Cover tilt and sheen" effect on, the box leans toward a mouse or
 * pen pointer and a soft highlight follows it across the cover. The motion
 * runs on CSS variables written once per animation frame, so moving the
 * pointer never re-renders React. Touch input is ignored (a tilting cover
 * under a finger fights scrolling and long-press), and the effect is off
 * under reduced motion via `useVisualEffect`.
 *
 * With the effect off this is a plain `div`, so cards look exactly as before.
 */
export const CoverTilt = forwardRef<
  HTMLDivElement,
  React.HTMLAttributes<HTMLDivElement>
>(function CoverTilt({ className, children, ...rest }, forwardedRef) {
  const on = useVisualEffect("cover_tilt");
  const box = useRef<HTMLDivElement | null>(null);
  const frame = useRef<number | null>(null);
  const target = useRef({ x: 0.5, y: 0.5 });

  const setRefs = useCallback(
    (el: HTMLDivElement | null) => {
      box.current = el;
      if (typeof forwardedRef === "function") forwardedRef(el);
      else if (forwardedRef) forwardedRef.current = el;
    },
    [forwardedRef],
  );

  useEffect(
    () => () => {
      if (frame.current !== null) cancelAnimationFrame(frame.current);
    },
    [],
  );

  if (!on) {
    return (
      <div ref={setRefs} className={className} {...rest}>
        {children}
      </div>
    );
  }

  const paint = () => {
    frame.current = null;
    const el = box.current;
    if (!el) return;
    const { x, y } = target.current;
    el.style.setProperty(
      "--tilt-x",
      `${((0.5 - y) * 2 * MAX_TILT_DEG).toFixed(2)}deg`,
    );
    el.style.setProperty(
      "--tilt-y",
      `${((x - 0.5) * 2 * MAX_TILT_DEG).toFixed(2)}deg`,
    );
    el.style.setProperty("--sheen-x", `${(x * 100).toFixed(1)}%`);
    el.style.setProperty("--sheen-y", `${(y * 100).toFixed(1)}%`);
  };

  const onPointerMove = (e: React.PointerEvent<HTMLDivElement>) => {
    rest.onPointerMove?.(e);
    if (e.pointerType === "touch" || !box.current) return;
    const r = box.current.getBoundingClientRect();
    if (r.width === 0 || r.height === 0) return;
    target.current = {
      x: Math.min(1, Math.max(0, (e.clientX - r.left) / r.width)),
      y: Math.min(1, Math.max(0, (e.clientY - r.top) / r.height)),
    };
    box.current.dataset.tilting = "";
    if (frame.current === null) frame.current = requestAnimationFrame(paint);
  };

  const onPointerLeave = (e: React.PointerEvent<HTMLDivElement>) => {
    rest.onPointerLeave?.(e);
    const el = box.current;
    if (!el) return;
    if (frame.current !== null) cancelAnimationFrame(frame.current);
    frame.current = null;
    delete el.dataset.tilting;
    el.style.setProperty("--tilt-x", "0deg");
    el.style.setProperty("--tilt-y", "0deg");
  };

  return (
    <div
      ref={setRefs}
      {...rest}
      onPointerMove={onPointerMove}
      onPointerLeave={onPointerLeave}
      className={cn("cover-tilt", className)}
    >
      {children}
      <span aria-hidden className="cover-tilt-sheen" />
    </div>
  );
});
