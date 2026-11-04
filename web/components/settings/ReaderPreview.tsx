"use client";

import { useEffect, useState } from "react";

import { useContinueReading } from "@/lib/api/queries";
import { pageThumbUrl } from "@/lib/urls";
import { cn } from "@/lib/utils";
import { useReducedMotion } from "@/lib/visual-effects";

export type PreviewPrefs = {
  direction: "auto" | "ltr" | "rtl";
  fit: "auto" | "width" | "height" | "original" | "contain";
  view: "auto" | "single" | "double" | "webtoon";
  animation: "off" | "slide" | "fade";
  strip: boolean;
  coverSolo: boolean;
};

/** Pages the preview flips through. */
const PAGES = 7;
/** Time each spread stays up before the preview turns the page. */
const TURN_MS = 2200;

/** Pure: the spreads a view shows, in reading order. Exported for tests. */
export function previewSpreads(
  view: PreviewPrefs["view"],
  coverSolo: boolean,
  pages = PAGES,
): number[][] {
  const all = Array.from({ length: pages }, (_, i) => i);
  if (view !== "double") return all.map((p) => [p]);
  const out: number[][] = [];
  let i = 0;
  if (coverSolo) {
    out.push([0]);
    i = 1;
  }
  for (; i < pages; i += 2) out.push(all.slice(i, i + 2));
  return out;
}

const LABEL = {
  direction: {
    auto: "Auto direction",
    ltr: "Left → right",
    rtl: "Right → left",
  },
  fit: {
    auto: "Auto fit",
    width: "Fit width",
    height: "Fit height",
    original: "Original size",
    contain: "Fit screen",
  },
  view: {
    auto: "Auto view",
    single: "Single",
    double: "Double",
    webtoon: "Webtoon",
  },
} as const;

/**
 * "Live setting previews" (folio-flair-1.0). A small reader drawn with the
 * pages of the issue you're reading (placeholder pages if none), redrawn
 * as the reading defaults change: direction, fit, view, the page-turn
 * animation (it turns a page every couple of seconds), the page strip and
 * cover-solo spreads. Auto settings preview the reader's usual fallback
 * (single page, left to right, fit screen). Under reduced motion it stays
 * on one spread.
 */
export function ReaderPreview({ prefs }: { prefs: PreviewPrefs }) {
  const reduced = useReducedMotion();
  const cont = useContinueReading();
  const issue = cont.data?.items[0]?.issue ?? null;
  const pageCount = Math.min(PAGES, issue?.page_count ?? PAGES);

  const view = prefs.view === "auto" ? "single" : prefs.view;
  const rtl = prefs.direction === "rtl";
  const spreads = previewSpreads(view, prefs.coverSolo, pageCount);
  const [turn, setTurn] = useState(0);
  const at = spreads.length ? turn % spreads.length : 0;
  const spread = spreads[at] ?? [0];

  useEffect(() => {
    if (reduced || view === "webtoon") return;
    const id = setInterval(() => setTurn((t) => t + 1), TURN_MS);
    return () => clearInterval(id);
  }, [reduced, view]);

  const src = (p: number) =>
    issue ? `${pageThumbUrl(issue.id, p)}?variant=strip` : null;

  const fit = prefs.fit === "auto" ? "contain" : prefs.fit;
  const enter =
    reduced || prefs.animation === "off"
      ? ""
      : prefs.animation === "fade"
        ? "reader-preview-fade"
        : rtl
          ? "reader-preview-slide-rtl"
          : "reader-preview-slide";

  return (
    <figure className="space-y-2" aria-label="Reader preview">
      <div className="text-muted-foreground flex items-center justify-between text-xs font-medium tracking-wide uppercase">
        <span>Preview</span>
        <span className="bg-primary/15 text-primary rounded px-1.5 py-0.5 text-[10px]">
          Live
        </span>
      </div>
      <div
        aria-hidden
        className="relative aspect-[16/10] overflow-hidden rounded-lg border bg-black shadow-inner"
      >
        {view === "webtoon" ? (
          <div
            className={cn(
              "mx-auto flex w-3/5 flex-col",
              !reduced && "reader-preview-scroll",
            )}
          >
            {[...Array(pageCount * 2)].map((_, i) => (
              <PreviewPage key={i} src={src(i % pageCount)} n={i % pageCount} />
            ))}
          </div>
        ) : (
          <div
            key={`${at}-${spread.join()}`}
            className={cn(
              "absolute inset-0 flex justify-center",
              rtl && "flex-row-reverse",
              fit === "width" ? "items-start" : "items-center",
              enter,
            )}
          >
            {spread.map((p) => (
              <div
                key={p}
                className={cn(
                  "shrink-0",
                  fit === "width" && (spread.length > 1 ? "w-1/2" : "w-full"),
                  (fit === "height" || fit === "contain") && "h-full",
                  fit === "original" && "h-[72%]",
                )}
              >
                <PreviewPage src={src(p)} n={p} fill={fit !== "width"} />
              </div>
            ))}
          </div>
        )}
        {prefs.strip && view !== "webtoon" ? (
          <div
            className={cn(
              "absolute inset-x-0 bottom-0 flex justify-center gap-1 bg-black/70 p-1",
              rtl && "flex-row-reverse",
            )}
          >
            {Array.from({ length: pageCount }, (_, p) => (
              <div
                key={p}
                className={cn(
                  "w-[7%] rounded-[2px] ring-1 transition-[box-shadow,opacity] duration-300",
                  spread.includes(p)
                    ? "ring-primary opacity-100"
                    : "opacity-50 ring-transparent",
                )}
              >
                <PreviewPage src={src(p)} n={p} />
              </div>
            ))}
          </div>
        ) : null}
        <span className="absolute top-1.5 right-2 rounded bg-black/60 px-1.5 text-[10px] text-white/80 tabular-nums">
          {view === "webtoon"
            ? "scroll"
            : `${spread.map((p) => p + 1).join("–")} / ${pageCount}`}
        </span>
      </div>
      <figcaption className="text-muted-foreground text-xs">
        {[
          LABEL.view[prefs.view],
          LABEL.fit[prefs.fit],
          LABEL.direction[prefs.direction],
        ].join(" · ")}
        {issue ? null : " · sample pages"}
      </figcaption>
    </figure>
  );
}

function PreviewPage({
  src,
  n,
  fill = false,
}: {
  src: string | null;
  n: number;
  fill?: boolean;
}) {
  if (!src) {
    return (
      <div
        className={cn(
          "flex aspect-[2/3] items-center justify-center bg-neutral-800 text-[10px] text-neutral-400",
          fill && "h-full",
        )}
      >
        {n + 1}
      </div>
    );
  }
  return (
    // eslint-disable-next-line @next/next/no-img-element
    <img
      src={src}
      alt=""
      decoding="async"
      className={cn(
        "block aspect-[2/3] bg-neutral-800 object-cover",
        fill ? "h-full w-auto" : "w-full",
      )}
    />
  );
}
