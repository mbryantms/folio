"use client";

import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { Loader2, RotateCw } from "lucide-react";

/** Show the "still loading…" hint after this long in flight (audit C3). */
const SLOW_HINT_MS = 8000;

/**
 * Page image with a centered spinner while the bytes are in flight. The
 * caller keys this component on issue+page so React unmounts on page-flip
 * and the loading state resets cleanly.
 *
 * Cached pages take a synchronous `complete` check in `useLayoutEffect`
 * so they paint at full opacity on the first frame — no fade, no spinner
 * blip — while genuinely-loading pages still show the spinner. Browsers
 * fire `load` events lazily for already-decoded images, so without the
 * layout-effect probe we'd race and flash empty for one frame.
 *
 * `onNaturalSize` is the marker-overlay hook: when the image's natural
 * (intrinsic) pixel dimensions are known, the parent caches them so the
 * marker overlay can sample the source image at native resolution for
 * OCR / image-hash workflows.
 */
export function PageImage({
  src,
  alt,
  fitClass,
  loading,
  fetchPriority,
  onNaturalSize,
  imgRef: externalImgRef,
  dimensions,
}: {
  src: string;
  alt: string;
  fitClass: string;
  loading?: "eager" | "lazy";
  /** Browser fetch priority. The visible page passes `"high"` so it never
   *  queues behind the low-priority prefetch warms. */
  fetchPriority?: "high" | "low" | "auto";
  onNaturalSize?: (width: number, height: number) => void;
  /** Optional ref the parent can use to align overlays (marker SVG)
   *  with the actually-rendered image bounds rather than the wider
   *  flex wrapper. Critical at fit=height where the image is narrower
   *  than the row container and centered inside it. */
  imgRef?: React.RefObject<HTMLImageElement | null>;
  /** Server-known intrinsic page dimensions. Rendered as the img's
   *  `width`/`height` attributes so the browser reserves layout
   *  height before the bytes arrive (the attributes only feed the
   *  aspect ratio — the fit classes still control rendered size, and
   *  the intrinsic ratio takes over on load, so a wrong hint can't
   *  distort the image). Webtoon mode passes this so a 200-page
   *  chapter doesn't mount as 200 zero-height images — which
   *  defeated `loading="lazy"` (everything sat inside the lazy
   *  margin), broke resume-scroll positioning, and let the
   *  most-visible-page observer persist regressed progress while
   *  decode shifted layout. */
  dimensions?: { width: number; height: number };
}) {
  // Load lifecycle (audit C3): loading → loaded | error. A failed load
  // auto-retries once silently (transient blips), then surfaces a
  // tap-to-retry state; a slow load shows a "still loading…" hint.
  const [status, setStatus] = useState<"loading" | "loaded" | "error">(
    "loading",
  );
  const [retry, setRetry] = useState(0);
  const [slow, setSlow] = useState(false);
  const autoRetried = useRef(false);
  const internalImgRef = useRef<HTMLImageElement>(null);
  const imgRef = externalImgRef ?? internalImgRef;
  const loaded = status === "loaded";

  // Cache-busted src so a retry actually re-fetches the failed bytes.
  const effectiveSrc =
    retry > 0 ? `${src}${src.includes("?") ? "&" : "?"}r=${retry}` : src;

  useLayoutEffect(() => {
    const el = imgRef.current;
    if (el && el.complete && el.naturalWidth > 0) {
      setStatus("loaded");
      onNaturalSize?.(el.naturalWidth, el.naturalHeight);
    }
    // `onNaturalSize` identity changes are tolerated — we only emit
    // once per image regardless, and a stale closure won't mis-report.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Surface a "still loading…" hint while genuinely in flight; reset on
  // each (re)try.
  useEffect(() => {
    if (status !== "loading") return;
    const t = setTimeout(() => setSlow(true), SLOW_HINT_MS);
    return () => clearTimeout(t);
  }, [status, retry]);

  const handleError = () => {
    if (!autoRetried.current) {
      autoRetried.current = true;
      setSlow(false);
      setRetry((n) => n + 1);
      return;
    }
    setStatus("error");
  };
  const handleRetry = () => {
    autoRetried.current = false;
    setSlow(false);
    setStatus("loading");
    setRetry((n) => n + 1);
  };

  return (
    // `flex w-full justify-center` so the wrapper is a real, viewport-width
    // container instead of an `inline-block` that sizes to the img — that
    // would make `w-full` on the img collapse to its natural width and
    // break "fit width" / "fit height" / "original" entirely. The flex +
    // justify-center keeps a narrower image (original or height mode)
    // centered horizontally inside the row.
    <span className="relative flex w-full justify-center">
      {status === "loading" ? (
        <span className="pointer-events-none absolute top-1/2 left-1/2 flex -translate-x-1/2 -translate-y-1/2 flex-col items-center gap-2 text-neutral-500">
          <Loader2 aria-hidden="true" className="size-8 animate-spin" />
          {slow ? (
            <span role="status" className="text-xs">
              Still loading…
            </span>
          ) : null}
        </span>
      ) : null}
      {status === "error" ? (
        // Failed after the silent auto-retry — offer a manual retry
        // rather than leaving a broken-image glyph (audit C3). The button
        // is the actionable target; the whole region is also clickable.
        <button
          type="button"
          onClick={handleRetry}
          className="text-muted-foreground hover:text-foreground absolute top-1/2 left-1/2 flex min-h-11 -translate-x-1/2 -translate-y-1/2 flex-col items-center gap-2 rounded-md px-4 py-3 text-sm"
        >
          <RotateCw aria-hidden="true" className="size-6" />
          <span>Couldn&apos;t load this page. Tap to retry.</span>
        </button>
      ) : null}
      {/* eslint-disable-next-line @next/next/no-img-element */}
      <img
        ref={imgRef}
        src={effectiveSrc}
        alt={alt}
        loading={loading}
        fetchPriority={fetchPriority}
        decoding="async"
        onLoad={(e) => {
          setSlow(false);
          setStatus("loaded");
          const img = e.currentTarget;
          if (img.naturalWidth > 0) {
            onNaturalSize?.(img.naturalWidth, img.naturalHeight);
          }
        }}
        onError={handleError}
        // v0.3.44 entrance polish: fresh-load images fade in over
        // 150ms so the spinner-to-image transition has visual
        // continuity instead of a hard swap. Cached/already-decoded
        // images set `loaded=true` synchronously in the
        // useLayoutEffect above, so they paint at full opacity on
        // the first frame (no flash). `motion-reduce` honors
        // `prefers-reduced-motion`.
        width={dimensions?.width}
        height={dimensions?.height}
        className={`block ${fitClass} transition-opacity duration-150 ease-out motion-reduce:transition-none ${
          loaded ? "opacity-100" : "opacity-0"
        }`}
      />
    </span>
  );
}
