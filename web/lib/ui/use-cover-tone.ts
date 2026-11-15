"use client";

import { useCallback, useLayoutEffect, useRef } from "react";

import { applyTint, getTone, rememberTone } from "@/lib/ui/cover-tone";

/**
 * The cover's colour, for the component that draws a `.cover-box`
 * (`<Cover>`, `<UpcomingCover>`); one of the three parts of the cover
 * frame (`components/library/CoverFrame.tsx`).
 *
 * Put `boxRef` on the box. Before first paint the box takes the cover's
 * remembered average colour as its placeholder and the enclosing card
 * gets `--cover-tint` (both set on the DOM: no extra render and no SSR
 * mismatch). Call `remember(img)` once the image has decoded so the
 * colours are there next time.
 */
export function useCoverTone(src: string | null | undefined): {
  boxRef: React.RefObject<HTMLDivElement | null>;
  remember: (img: HTMLImageElement | null) => void;
} {
  const boxRef = useRef<HTMLDivElement | null>(null);

  useLayoutEffect(() => {
    if (!src) return;
    const tone = getTone(src);
    if (!tone) return;
    if (boxRef.current) {
      boxRef.current.style.backgroundColor = `rgb(${tone.average})`;
    }
    applyTint(boxRef.current, tone);
  }, [src]);

  const remember = useCallback(
    (img: HTMLImageElement | null) => {
      if (src && img) rememberTone(src, img);
    },
    [src],
  );

  return { boxRef, remember };
}
