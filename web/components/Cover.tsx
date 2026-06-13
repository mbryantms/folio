"use client";

import { useLayoutEffect, useRef, useState } from "react";

import { useCoverPriority } from "@/components/library/cover-priority";

/**
 * Single-cover component used by the library grid, rails, series page,
 * issue page.
 *
 * The image fades in once decoded (over a stable dark placeholder box)
 * instead of painting onto the page as bytes arrive — otherwise a grid of
 * lazy covers reads as a cascade of blank tiles filling top-to-bottom. A
 * cached cover is detected synchronously in `useLayoutEffect` (via
 * `complete`), so it paints at full opacity on the first frame — no fade,
 * no flash. Mirrors the reader's `PageImage`.
 *
 * Falls back to a placeholder tile (publisher / state) when `src` is null —
 * keeps the layout stable for issues with no cover (encrypted, malformed,
 * or thumbnail not yet generated).
 */
// Theme tokens, not hardcoded neutrals: the old neutral-600-on-
// neutral-900 fallback text measured ≈2.3:1 (WCAG 1.4.3 wants 4.5:1)
// and painted a near-black tile on the light/amber themes.
const BOX =
  "aspect-[2/3] bg-muted rounded-md border border-border overflow-hidden";

export function Cover({
  src,
  alt,
  fallback,
  className,
}: {
  src: string | null | undefined;
  alt: string;
  fallback?: string | null;
  className?: string;
}) {
  // Above-the-fold rails flag their subtree (see `CoverPriorityProvider`).
  // A prioritized cover eager-loads at high fetch priority and skips the
  // fade — it's the LCP candidate, so paint it as soon as it decodes.
  const priority = useCoverPriority();
  const [loaded, setLoaded] = useState(priority);
  const ref = useRef<HTMLImageElement>(null);

  useLayoutEffect(() => {
    const el = ref.current;
    if (el && el.complete && el.naturalWidth > 0) setLoaded(true);
  }, []);

  if (src) {
    return (
      // The box owns the aspect-ratio + placeholder + caller classes
      // (sizing, hover brightness); the img fills it and fades in, so the
      // dark placeholder stays visible underneath until the cover decodes.
      <div className={`${BOX} ${className ?? ""}`}>
        {/* eslint-disable-next-line @next/next/no-img-element */}
        <img
          ref={ref}
          src={src}
          alt={alt}
          loading={priority ? "eager" : "lazy"}
          fetchPriority={priority ? "high" : undefined}
          decoding="async"
          onLoad={() => setLoaded(true)}
          onError={() => setLoaded(true)}
          className={`block h-full w-full object-cover transition-opacity duration-200 ease-out motion-reduce:transition-none ${
            loaded ? "opacity-100" : "opacity-0"
          }`}
        />
      </div>
    );
  }
  return (
    <div
      role="img"
      aria-label={alt}
      className={`${BOX} text-muted-foreground grid place-items-center ${className ?? ""}`}
    >
      <span className="px-2 text-center text-xs tracking-widest uppercase">
        {fallback ?? "—"}
      </span>
    </div>
  );
}
