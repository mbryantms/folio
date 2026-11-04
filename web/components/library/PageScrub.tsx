"use client";

import { useEffect, useRef, useState } from "react";

import { pageThumbUrl } from "@/lib/urls";
import { useVisualEffect } from "@/lib/visual-effects";

/** Frames shown across a cover's width. */
const FRAMES = 6;
/** The pointer must rest this long before any page is fetched, so moving
 *  across a grid doesn't request thumbnails for every card it passes. */
const DWELL_MS = 220;

/** Pages to show: the cover, then evenly spaced interior pages. */
export function scrubPages(pageCount: number, frames = FRAMES): number[] {
  if (!Number.isFinite(pageCount) || pageCount < 2) return [0];
  const last = Math.max(1, Math.floor(pageCount) - 1);
  const n = Math.min(frames, last + 1);
  const out = new Set<number>([0]);
  for (let i = 1; i < n; i++) out.add(Math.round((i * last) / (n - 1)));
  return [...out].sort((a, b) => a - b);
}

/** The issue id inside a `/issues/{id}/pages/0/thumb` cover URL. */
export function issueIdFromCoverUrl(
  url: string | null | undefined,
): string | null {
  const m = url?.match(/\/issues\/([^/]+)\/pages\/0\/thumb/);
  return m?.[1] ? decodeURIComponent(m[1]) : null;
}

/**
 * "Flip through pages on hover" (folio-flair-1.0, optional, off by default).
 *
 * Rendered inside a card's cover box (`CoverTilt`). With a mouse or pen
 * resting on the cover, moving across it shows a few of the issue's pages
 * in order, with a page marker along the bottom. It uses the small `strip`
 * page thumbnails and fetches nothing until the pointer has rested on the
 * cover for a moment. Touch is ignored; with the effect off it renders
 * nothing.
 */
export function PageScrub({
  issueId,
  pageCount,
}: {
  issueId: string | null | undefined;
  /** Page count of the issue; unknown counts fall back to a short range. */
  pageCount: number | null | undefined;
}) {
  const on = useVisualEffect("page_scrub");
  const [active, setActive] = useState(false);
  const [frame, setFrame] = useState(0);
  const [failed, setFailed] = useState<ReadonlySet<number>>(new Set());
  const dwell = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(
    () => () => {
      if (dwell.current) clearTimeout(dwell.current);
    },
    [],
  );

  if (!on || !issueId) return null;

  // Unknown count (series cards): assume a typical single issue; frames
  // past the real end 404 and are skipped.
  const pages = scrubPages(pageCount ?? 22).filter((p) => !failed.has(p));
  const url = (p: number) => `${pageThumbUrl(issueId, p)}?variant=strip`;

  const onPointerEnter = (e: React.PointerEvent) => {
    if (e.pointerType === "touch") return;
    if (dwell.current) clearTimeout(dwell.current);
    dwell.current = setTimeout(() => setActive(true), DWELL_MS);
  };
  const onPointerMove = (e: React.PointerEvent<HTMLDivElement>) => {
    if (e.pointerType === "touch" || pages.length < 2) return;
    const r = e.currentTarget.getBoundingClientRect();
    const x = Math.min(0.999, Math.max(0, (e.clientX - r.left) / r.width));
    setFrame(Math.floor(x * pages.length));
  };
  const onPointerLeave = () => {
    if (dwell.current) clearTimeout(dwell.current);
    dwell.current = null;
    setActive(false);
    setFrame(0);
  };

  const page = pages[Math.min(frame, pages.length - 1)] ?? 0;

  return (
    <div
      aria-hidden
      className="absolute inset-x-0 top-0 z-[4] aspect-[2/3] overflow-hidden rounded-md"
      onPointerEnter={onPointerEnter}
      onPointerMove={onPointerMove}
      onPointerLeave={onPointerLeave}
    >
      {active &&
        pages.map((p) => (
          // All frames mount at once (hidden) so moving between them is
          // instant; only the current one is visible.
          // eslint-disable-next-line @next/next/no-img-element
          <img
            key={p}
            src={url(p)}
            alt=""
            decoding="async"
            onError={() => setFailed((s) => new Set(s).add(p))}
            className={`bg-muted absolute inset-0 h-full w-full object-contain transition-opacity duration-100 ${
              p === page && p !== 0 ? "opacity-100" : "opacity-0"
            }`}
          />
        ))}
      {active && pages.length > 1 && (
        <div className="absolute right-12 bottom-2 left-2 flex gap-1">
          {pages.map((p) => (
            <span
              key={p}
              className={`h-1 flex-1 rounded-full shadow-sm transition-colors ${
                p === page ? "bg-primary" : "bg-white/50"
              }`}
            />
          ))}
        </div>
      )}
    </div>
  );
}
