"use client";

import { useEffect, useRef, useState } from "react";

import { Progress } from "@/components/ui/progress";
import { useReducedMotion } from "@/lib/visual-effects";

/**
 * A progress bar that fills from empty when it first scrolls into view,
 * in step with the `CountUp` numbers beside it (folio-flair-1.0, count-up
 * stats). Reduced motion shows the final value at once.
 */
export function FillProgress({
  value,
  label,
}: {
  value: number;
  label: string;
}) {
  const reduced = useReducedMotion();
  const ref = useRef<HTMLDivElement>(null);
  const [shown, setShown] = useState(value);
  const [animate, setAnimate] = useState(false);

  useEffect(() => {
    const el = ref.current;
    if (
      !el ||
      reduced ||
      value <= 0 ||
      typeof IntersectionObserver === "undefined"
    )
      return;
    setShown(0);
    let raf = 0;
    const io = new IntersectionObserver((entries) => {
      if (!entries.some((e) => e.isIntersecting)) return;
      io.disconnect();
      raf = requestAnimationFrame(() => {
        setAnimate(true);
        setShown(value);
      });
    });
    io.observe(el);
    return () => {
      io.disconnect();
      cancelAnimationFrame(raf);
    };
  }, [value, reduced]);

  return (
    <div ref={ref}>
      <Progress
        value={shown}
        aria-label={label}
        className={
          animate
            ? "[&_[data-slot=progress-indicator]]:duration-[1200ms] [&_[data-slot=progress-indicator]]:ease-out"
            : undefined
        }
      />
    </div>
  );
}
