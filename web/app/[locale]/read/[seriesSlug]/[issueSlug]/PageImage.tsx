"use client";

import { useLayoutEffect, useRef, useState } from "react";
import { Loader2 } from "lucide-react";

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
  onNaturalSize,
  imgRef: externalImgRef,
}: {
  src: string;
  alt: string;
  fitClass: string;
  loading?: "eager" | "lazy";
  onNaturalSize?: (width: number, height: number) => void;
  /** Optional ref the parent can use to align overlays (marker SVG)
   *  with the actually-rendered image bounds rather than the wider
   *  flex wrapper. Critical at fit=height where the image is narrower
   *  than the row container and centered inside it. */
  imgRef?: React.RefObject<HTMLImageElement | null>;
}) {
  const [loaded, setLoaded] = useState(false);
  const internalImgRef = useRef<HTMLImageElement>(null);
  const imgRef = externalImgRef ?? internalImgRef;

  useLayoutEffect(() => {
    const el = imgRef.current;
    if (el && el.complete && el.naturalWidth > 0) {
      setLoaded(true);
      onNaturalSize?.(el.naturalWidth, el.naturalHeight);
    }
    // `onNaturalSize` identity changes are tolerated — we only emit
    // once per image regardless, and a stale closure won't mis-report.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return (
    // `flex w-full justify-center` so the wrapper is a real, viewport-width
    // container instead of an `inline-block` that sizes to the img — that
    // would make `w-full` on the img collapse to its natural width and
    // break "fit width" / "fit height" / "original" entirely. The flex +
    // justify-center keeps a narrower image (original or height mode)
    // centered horizontally inside the row.
    <span className="relative flex w-full justify-center">
      {!loaded ? (
        <Loader2
          aria-hidden="true"
          className="pointer-events-none absolute top-1/2 left-1/2 size-8 -translate-x-1/2 -translate-y-1/2 animate-spin text-neutral-600"
        />
      ) : null}
      {/* eslint-disable-next-line @next/next/no-img-element */}
      <img
        ref={imgRef}
        src={src}
        alt={alt}
        loading={loading}
        decoding="async"
        onLoad={(e) => {
          setLoaded(true);
          const img = e.currentTarget;
          if (img.naturalWidth > 0) {
            onNaturalSize?.(img.naturalWidth, img.naturalHeight);
          }
        }}
        onError={() => setLoaded(true)}
        className={`block ${fitClass}`}
      />
    </span>
  );
}
