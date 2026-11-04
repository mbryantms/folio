"use client";

import type { PageInfo } from "@/lib/api/types";
import type { Direction } from "@/lib/reader/detect";
import { pageBytesSrcSet, withContentVersion } from "@/lib/urls";

import { PageImage } from "./PageImage";

/** The neighbouring page(s) either side of the current one, shown while
 *  a drag-to-turn is in progress (`use-drag-turn.ts`). `null` = nothing
 *  that way (first / last page: the drag rubber-bands instead). Visual
 *  sides, already resolved for the reading direction. */
export type DragPeekPages = {
  left: readonly number[] | null;
  right: readonly number[] | null;
};

/**
 * One off-screen slot beside the current page inside the drag track: a
 * full-width box one viewport to the left or right, holding the page (or
 * spread) the drag reveals. It renders the same `PageImage` the view
 * will mount after the turn — same URL, srcset and sizes, so the bytes
 * the prefetcher warmed are reused and the swap at the end of the turn
 * is seamless.
 */
export function DragTurnPeek({
  side,
  pages,
  issueId,
  pageInfos,
  pageUrlVersion,
  direction,
  fitClass,
  paneClass,
  sizes,
  variants,
}: {
  side: "left" | "right";
  pages: readonly number[];
  issueId: string;
  pageInfos: readonly PageInfo[];
  pageUrlVersion: string | null;
  direction: Direction;
  fitClass: string;
  /** Double view: the pane wrapper class each page gets there. */
  paneClass?: string;
  sizes: string;
  /** Width-negotiated srcset on (off for original-fit). */
  variants: boolean;
}) {
  const flex =
    paneClass !== undefined && direction === "rtl"
      ? "flex-row-reverse"
      : "flex-row";
  return (
    <div
      aria-hidden="true"
      data-drag-peek={side}
      className={`pointer-events-none absolute inset-y-0 flex w-full items-center justify-center gap-1 ${flex}`}
      style={{ left: side === "left" ? "-100%" : "100%" }}
    >
      {pages.map((p) => {
        const src = withContentVersion(
          `/issues/${issueId}/pages/${p}`,
          pageUrlVersion,
        );
        const img = (
          <PageImage
            key={p}
            src={src}
            srcSet={
              variants
                ? pageBytesSrcSet(src, pageInfos[p]?.image_width)
                : undefined
            }
            sizes={sizes}
            thumbSrc={withContentVersion(
              `/issues/${issueId}/pages/${p}/thumb?variant=strip`,
              pageUrlVersion,
            )}
            alt=""
            fitClass={fitClass}
            dimensions={
              pageInfos[p]?.image_width && pageInfos[p]?.image_height
                ? {
                    width: pageInfos[p]!.image_width!,
                    height: pageInfos[p]!.image_height!,
                  }
                : undefined
            }
          />
        );
        return paneClass !== undefined ? (
          <div key={p} className={`relative align-top ${paneClass}`}>
            {img}
          </div>
        ) : (
          <div key={p} className="w-full">
            {img}
          </div>
        );
      })}
    </div>
  );
}
