"use client";

import { useLayoutEffect, useRef, useState } from "react";

import { useCoverPriority } from "@/components/library/cover-priority";
import { RetryingImg } from "@/components/RetryingImg";
import { applyTint, getTone, rememberTone } from "@/lib/ui/cover-tone";
import { COVER_CARD_SIZES } from "@/lib/library/cover-size";
import { coverThumbSrcSet } from "@/lib/urls";

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
 * Blur-up (folio-flair-1.0): while a cover loads, its box shows the
 * cover's own average colour when we have seen it before (`cover-tone`),
 * and the image resolves from a soft blur as it fades in. Once decoded,
 * its colours are remembered and the card gets a `--cover-tint` for its
 * hover wash.
 *
 * Falls back to a placeholder tile (publisher / state) when `src` is null —
 * keeps the layout stable for issues with no cover (encrypted, malformed,
 * or thumbnail not yet generated).
 */
// Theme tokens, not hardcoded neutrals: the old neutral-600-on-
// neutral-900 fallback text measured ≈2.3:1 (WCAG 1.4.3 wants 4.5:1)
// and painted a near-black tile on the light/amber themes.
const BOX =
  "cover-box aspect-[2/3] bg-muted rounded-md border border-border overflow-hidden";

/** The `sizes` attribute for a cover: none without a `srcset`, `auto` for
 *  lazy covers (the browser measures the box), the static card-width hint
 *  for eager priority covers, where `auto` does not apply. */
export function coverSizes(
  srcSet: string | undefined,
  priority: boolean,
): string | undefined {
  if (!srcSet) return undefined;
  return priority ? COVER_CARD_SIZES : "auto";
}

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
  const boxRef = useRef<HTMLDivElement>(null);

  useLayoutEffect(() => {
    const el = ref.current;
    if (el && el.complete && el.naturalWidth > 0) {
      setLoaded(true);
      if (src) rememberTone(src, el);
    }
    // Mount-only: `src` changes re-key the img via onLoad.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Before first paint: the remembered colour as the placeholder, and the
  // card's tint (set on the DOM, so no extra render and no SSR mismatch).
  useLayoutEffect(() => {
    if (!src) return;
    const tone = getTone(src);
    if (!tone) return;
    if (boxRef.current) {
      boxRef.current.style.backgroundColor = `rgb(${tone.average})`;
    }
    applyTint(boxRef.current, tone);
  }, [src]);

  const onLoad = () => {
    setLoaded(true);
    if (src && ref.current) rememberTone(src, ref.current);
  };

  if (src) {
    // Responsive cover (G9): lazy covers get a `srcset` + `sizes="auto"`
    // so the browser picks the small (300px) variant for low-DPR / small-card
    // renders instead of always pulling the full 600px. `auto` is only
    // honoured on lazy images, so priority (eager, LCP) covers — only ever
    // the first home rail's cards — take the static card-width hint
    // `COVER_CARD_SIZES` instead of defaulting to the viewport width.
    // Non-thumb URLs (provider covers) get a plain `src`.
    const srcSet = coverThumbSrcSet(src) ?? undefined;
    return (
      // The box owns the aspect-ratio + placeholder + caller classes
      // (sizing, hover brightness); the img fills it and fades in, so the
      // dark placeholder stays visible underneath until the cover decodes.
      <div ref={boxRef} className={`${BOX} ${className ?? ""}`}>
        {/* A failed load re-requests twice after a back-off (a 429 from
            the thumbnail bucket), then shows as failed. */}
        <RetryingImg
          ref={ref}
          src={src}
          srcSet={srcSet}
          sizes={coverSizes(srcSet, priority)}
          alt={alt}
          loading={priority ? "eager" : "lazy"}
          fetchPriority={priority ? "high" : undefined}
          decoding="async"
          onLoad={onLoad}
          onError={() => setLoaded(true)}
          className={`block h-full w-full object-cover transition-[opacity,filter,scale] duration-300 ease-out motion-reduce:transition-none ${
            loaded ? "opacity-100" : "scale-[1.04] opacity-0 blur-md"
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
