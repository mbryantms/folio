"use client";

import { useCallback, useLayoutEffect, useRef } from "react";

import {
  applyTint,
  decodeTone,
  getTone,
  rememberTone,
} from "@/lib/ui/cover-tone";

/**
 * The cover's colour, for the component that draws a `.cover-box`
 * (`<Cover>`, `<UpcomingCover>`); one of the three parts of the cover
 * frame (`components/library/CoverFrame.tsx`).
 *
 * Put `boxRef` on the box. Before first paint the box takes the cover's
 * average colour as its placeholder and the enclosing card gets
 * `--cover-tint` (both set on the DOM: no extra render and no SSR
 * mismatch). Call `remember(img)` once the image has decoded so the
 * colours are there next time.
 *
 * `known` is the colour pair the server sent with the cover (twelve hex
 * digits, `metadata::cover_tone`). It wins over this device's cache, and
 * it is the only source for a cross-origin cover the browser may not
 * sample, such as an upcoming release's.
 */
export function useCoverTone(
  src: string | null | undefined,
  known?: string | null,
): {
  boxRef: React.RefObject<HTMLDivElement | null>;
  remember: (img: HTMLImageElement | null) => void;
} {
  const boxRef = useRef<HTMLDivElement | null>(null);

  useLayoutEffect(() => {
    if (!src) return;
    const tone = decodeTone(known) ?? getTone(src);
    if (!tone) return;
    if (boxRef.current) {
      boxRef.current.style.backgroundColor = `rgb(${tone.average})`;
    }
    applyTint(boxRef.current, tone);
  }, [src, known]);

  const remember = useCallback(
    (img: HTMLImageElement | null) => {
      if (src && img) rememberTone(src, img);
    },
    [src],
  );

  return { boxRef, remember };
}
