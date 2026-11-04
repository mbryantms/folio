"use client";

import { useLayoutEffect, useRef } from "react";

import { formatCompactPages, formatReadingTimeCompact } from "@/lib/format";
import { useReducedMotion } from "@/lib/visual-effects";

const DURATION_MS = 1200;

/** Named formats (a server component can't pass a function prop). */
const FORMATS = {
  number: (n: number) => n.toLocaleString(),
  percent: (n: number) => `${n}%`,
  compactPages: (n: number) => formatCompactPages(n),
  /** `n` is a page count; shows the reading-time estimate for it. */
  readingTime: (n: number) => formatReadingTimeCompact(n) ?? "~<1m",
} as const;
export type CountUpFormat = keyof typeof FORMATS;

/** Ease-out cubic: fast start, gentle landing. */
export const easeOut = (t: number) => 1 - (1 - t) ** 3;

/**
 * A number that counts up from zero the first time it scrolls into view
 * (folio-flair-1.0 "Count-up stats", standard). The final value is in the
 * markup from the start, so server rendering, screen readers and reduced
 * motion all get the real number; only the visible digits animate, written
 * straight to the DOM once per frame.
 */
export function CountUp({
  value,
  format = "number",
  className,
}: {
  value: number;
  format?: CountUpFormat;
  className?: string;
}) {
  const ref = useRef<HTMLSpanElement>(null);
  const reduced = useReducedMotion();

  useLayoutEffect(() => {
    const el = ref.current;
    if (
      !el ||
      reduced ||
      value <= 0 ||
      typeof IntersectionObserver === "undefined"
    )
      return;
    const fmt = FORMATS[format];
    let raf = 0;
    el.textContent = fmt(0);
    el.classList.add("count-up-active");
    const run = () => {
      const start = performance.now();
      const step = (now: number) => {
        const t = Math.min(1, (now - start) / DURATION_MS);
        el.textContent = fmt(Math.round(value * easeOut(t)));
        if (t < 1) raf = requestAnimationFrame(step);
        else el.classList.remove("count-up-active");
      };
      raf = requestAnimationFrame(step);
    };
    const io = new IntersectionObserver((entries) => {
      if (entries.some((e) => e.isIntersecting)) {
        io.disconnect();
        run();
      }
    });
    io.observe(el);
    return () => {
      io.disconnect();
      cancelAnimationFrame(raf);
      el.classList.remove("count-up-active");
      el.textContent = fmt(value);
    };
  }, [value, reduced, format]);

  return (
    <span
      ref={ref}
      className={`count-up ${className ?? ""}`}
      style={{ fontVariantNumeric: "tabular-nums" }}
    >
      {FORMATS[format](value)}
    </span>
  );
}
