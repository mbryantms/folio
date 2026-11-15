"use client";

import { forwardRef, useCallback, useEffect, useRef } from "react";

import { CoverTilt, MAX_TILT_DEG } from "@/components/library/CoverTilt";
import { cn } from "@/lib/utils";

/** How far a frame leans. A card leans the full amount; the large hero
 *  cover on a series or issue page leans half as far, so a cover four
 *  times the area does not swing four times as much. */
export const COVER_LEAN = {
  card: MAX_TILT_DEG,
  hero: MAX_TILT_DEG / 2,
} as const;
export type CoverLean = keyof typeof COVER_LEAN;

/**
 * The cover frame: the positioned box that holds a card's cover and its
 * corner overlays (docs/dev/card-corner-conventions.md). Every cover card
 * uses it, so every cover behaves the same way:
 *
 *  - **tilt and sheen** toward a mouse or pen pointer, when the account's
 *    "Cover tilt and sheen" effect is on and motion is not reduced
 *    (`CoverTilt`); a plain box otherwise;
 *  - **the cover box** (`.cover-box`), drawn by the `<Cover>` or
 *    `<UpcomingCover>` inside it: the placeholder, the rounded border and
 *    the element the hover glow lands on;
 *  - **the cover's colour** (`useCoverTone`, called by those two): the
 *    placeholder takes the cover's average colour and the enclosing
 *    `[data-card]` gets `--cover-tint` for its hover wash and glow.
 *
 * The frame is `relative`, so overlays position against the cover. In
 * development it warns when nothing inside it draws a `.cover-box`, which
 * is how a new card finds out it left the cover's part out.
 *
 * Not for hover previews, fanned cover stacks or row thumbnails: those
 * stay still.
 */
export const CoverFrame = forwardRef<
  HTMLDivElement,
  React.HTMLAttributes<HTMLDivElement> & {
    /** `hero`: the gentler lean of a page's large header cover. */
    lean?: CoverLean;
  }
>(function CoverFrame({ lean = "card", className, ...rest }, forwardedRef) {
  const box = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    if (process.env.NODE_ENV === "production") return;
    if (box.current && !box.current.querySelector(".cover-box")) {
      console.warn(
        "CoverFrame: nothing inside draws a .cover-box. Draw the art with <Cover> (or <UpcomingCover>) so the card gets the placeholder colour and the hover glow.",
      );
    }
  }, []);

  const setRefs = useCallback(
    (el: HTMLDivElement | null) => {
      box.current = el;
      if (typeof forwardedRef === "function") forwardedRef(el);
      else if (forwardedRef) forwardedRef.current = el;
    },
    [forwardedRef],
  );

  return (
    <CoverTilt
      ref={setRefs}
      maxDeg={COVER_LEAN[lean]}
      data-cover-frame={lean}
      className={cn("relative", className)}
      {...rest}
    />
  );
});
