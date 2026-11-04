"use client";

import { useVisualEffect } from "@/lib/visual-effects";

/**
 * Stack thickness in px for a series of `count` issues: 0 for a one-shot,
 * 3 px for short runs, growing on a log scale to 12 px at ~200+.
 */
export function spineDepth(count: number | null | undefined): number {
  if (!count || count < 2) return 0;
  const t =
    3 + (9 * (Math.log(count) - Math.log(6))) / (Math.log(216) - Math.log(6));
  return Math.min(12, Math.max(3, t));
}

/**
 * "Series thickness" (folio-flair-1.0, optional, on by default): page edges
 * stacked behind a series card's cover, thicker for longer series. Drawn as
 * offset box-shadows on one element behind the cover, so it costs one
 * extra node per card. Needs an `isolate` (or transformed) parent so the
 * `-z-10` layer stays inside the card.
 */
export function SpineStack({ count }: { count: number | null | undefined }) {
  const on = useVisualEffect("spine_stack");
  const depth = spineDepth(count);
  if (!on || depth === 0) return null;
  const layers = Math.max(2, Math.round(depth / 1.6));
  const step = depth / layers;
  const shadows: string[] = [];
  for (let i = 1; i <= layers; i++) {
    shadows.push(
      `${(i * step).toFixed(2)}px ${(i * step * 0.7).toFixed(2)}px 0 var(${i % 2 ? "--spine-a" : "--spine-b"})`,
    );
  }
  shadows.push(
    `${(depth + 1).toFixed(1)}px ${(depth * 0.7 + 2).toFixed(1)}px 6px rgb(0 0 0 / 0.25)`,
  );
  return (
    <span
      aria-hidden
      className="spine-stack pointer-events-none absolute inset-x-0 top-0 -z-10 aspect-[2/3] rounded-md"
      style={{ boxShadow: shadows.join(",") }}
    />
  );
}
