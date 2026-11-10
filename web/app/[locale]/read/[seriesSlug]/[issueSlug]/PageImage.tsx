"use client";

import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type CSSProperties,
} from "react";
import { Loader2, RotateCw } from "lucide-react";
import { READER_FILTER_VAR } from "@/lib/reader/filters";

/** Show the "still loading…" hint after this long in flight (audit C3). */
const SLOW_HINT_MS = 8000;

/**
 * Where the failed-page retry sits, as a `top` offset (px, in the page
 * box's own coordinates) that puts it in the middle of the part of the box
 * on screen, so it stays in view however far a tall page is scrolled
 * (audit U-05). `position: fixed` can't do this: a zoomed or dragged page
 * carries a transform, which turns `fixed` back into box-relative.
 * `scale` converts screen px to the box's px under a zoom transform.
 */
export function retryTopPx(
  box: { top: number; bottom: number },
  viewportHeight: number,
  scale = 1,
): number {
  const top = Math.max(box.top, 0);
  const bottom = Math.min(box.bottom, viewportHeight);
  const middle = bottom > top ? (top + bottom) / 2 : (box.top + box.bottom) / 2;
  return (middle - box.top) / (scale > 0 ? scale : 1);
}

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
  srcSet,
  sizes,
  alt,
  lang,
  fitClass,
  loading,
  fetchPriority,
  onNaturalSize,
  imgRef: externalImgRef,
  dimensions,
  thumbSrc,
}: {
  src: string;
  /** Width-negotiated variant set (audit FEP-1; `pageBytesSrcSet`).
   *  Omitted for original-fit / pinch-zoom, where full-res is the point. */
  srcSet?: string;
  sizes?: string;
  alt: string;
  /** Language of the `alt` text when it quotes OCR'd page text
   *  (`usePageAltText`), so a screen reader voices it correctly. */
  lang?: string;
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
  /** Low-res page thumbnail (the page-strip variant — already warm in
   *  cache for nearby pages) shown blurred underneath while the full
   *  page decodes (audit C3 blur-up). Decorative: it shares the grid
   *  cell with the real image, so the loaded thumb also reserves the
   *  cell's height before the full bytes arrive. Omitted → plain
   *  spinner placeholder, as before. */
  thumbSrc?: string;
}) {
  // Load lifecycle (audit C3): loading → loaded | error. A failed load
  // auto-retries once silently (transient blips), then surfaces a
  // tap-to-retry state; a slow load shows a "still loading…" hint.
  const [status, setStatus] = useState<"loading" | "loaded" | "error">(
    "loading",
  );
  const [retry, setRetry] = useState(0);
  const [slow, setSlow] = useState(false);
  // Intrinsic aspect (w/h) once decoded — the fallback for `--page-ar`
  // when the scanner didn't record dimensions.
  const [naturalAspect, setNaturalAspect] = useState<number | null>(null);
  const autoRetried = useRef(false);
  const internalImgRef = useRef<HTMLImageElement>(null);
  const imgRef = externalImgRef ?? internalImgRef;
  const boxRef = useRef<HTMLSpanElement>(null);
  const retryRef = useRef<HTMLButtonElement>(null);
  const loaded = status === "loaded";

  // Cache-busted src so a retry actually re-fetches the failed bytes.
  const effectiveSrc =
    retry > 0 ? `${src}${src.includes("?") ? "&" : "?"}r=${retry}` : src;

  const handleError = () => {
    if (!autoRetried.current) {
      autoRetried.current = true;
      setSlow(false);
      setRetry((n) => n + 1);
      return;
    }
    setStatus("error");
  };

  useLayoutEffect(() => {
    const el = imgRef.current;
    if (!el?.complete) return;
    if (el.naturalWidth > 0) {
      setStatus("loaded");
      setNaturalAspect(el.naturalWidth / el.naturalHeight);
      onNaturalSize?.(el.naturalWidth, el.naturalHeight);
    } else {
      // A server-rendered image can also fail before hydration, when React
      // isn't listening for its `error` event yet.
      handleError();
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

  // Keep the retry in the visible part of the page while it is shown
  // (`retryTopPx`). Capture-phase scroll catches any scrolling ancestor.
  useEffect(() => {
    if (status !== "error") return;
    const box = boxRef.current;
    const button = retryRef.current;
    if (!box || !button) return;
    let frame = 0;
    const place = () => {
      frame = 0;
      const rect = box.getBoundingClientRect();
      const scale = box.offsetHeight > 0 ? rect.height / box.offsetHeight : 1;
      button.style.top = `${retryTopPx(rect, window.innerHeight, scale)}px`;
    };
    const schedule = () => {
      if (!frame) frame = requestAnimationFrame(place);
    };
    place();
    window.addEventListener("scroll", schedule, {
      passive: true,
      capture: true,
    });
    window.addEventListener("resize", schedule);
    return () => {
      cancelAnimationFrame(frame);
      window.removeEventListener("scroll", schedule, { capture: true });
      window.removeEventListener("resize", schedule);
    };
  }, [status]);

  const handleRetry = () => {
    autoRetried.current = false;
    setSlow(false);
    setStatus("loading");
    setRetry((n) => n + 1);
  };

  // Page aspect (w/h) exposed as `--page-ar` for the `contain` fit class
  // (WP-4.2), which sizes the img to `min(available width, safe viewport
  // height × aspect)` — fit-to-screen that also *up*-scales, without
  // `object-fit` (which would letterbox inside the img box and misalign
  // the marker overlay). Server dims first, decoded size as the fallback.
  const aspect =
    dimensions && dimensions.width > 0 && dimensions.height > 0
      ? dimensions.width / dimensions.height
      : naturalAspect;
  const aspectStyle =
    aspect && Number.isFinite(aspect)
      ? ({ "--page-ar": aspect } as CSSProperties)
      : undefined;

  return (
    // `grid w-full place-items-center` is a real, viewport-width container
    // (not an `inline-block` that sizes to the img — which would collapse
    // `w-full` and break the fit modes), and the single implicit cell lets
    // the blur-up thumb and the full image **stack** (`[&>img]:[grid-area:1/1]`)
    // and stay centered. Stacking is what makes the loaded thumb reserve
    // the cell's height while the full image (often 0-height until decode
    // at fit-width) catches up — no layout shift, no white gap.
    <span
      ref={boxRef}
      className="relative grid w-full place-items-center [&>img]:[grid-area:1/1]"
      style={aspectStyle}
    >
      {status === "loading" && (!thumbSrc || slow) ? (
        // `min(50%, 50dvh)`: the middle of the page, or of the screen when
        // the page is taller than it (fit-width), so this is never below
        // the fold.
        <span className="pointer-events-none absolute top-[min(50%,50dvh)] left-1/2 z-10 flex -translate-x-1/2 -translate-y-1/2 flex-col items-center gap-2 text-neutral-500">
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
        // rather than leaving a broken-image glyph (audit C3). z-20 puts it
        // over the reader's tap zones (z-10), which otherwise take the tap;
        // the rest of the page still turns. The attribute is what the cover
        // handoff watches for (ReaderCoverMorph.tsx). The class `top` is
        // the first-frame position; the effect above then pins it to the
        // visible part of the page.
        <button
          ref={retryRef}
          type="button"
          data-page-load-error=""
          onClick={handleRetry}
          className="bg-background/85 text-foreground hover:bg-background absolute top-[min(50%,50dvh)] left-1/2 z-20 flex min-h-11 -translate-x-1/2 -translate-y-1/2 flex-col items-center gap-2 rounded-md px-4 py-3 text-sm"
        >
          <RotateCw aria-hidden="true" className="size-6" />
          <span>Couldn&apos;t load this page. Tap to retry.</span>
        </button>
      ) : null}
      {thumbSrc ? (
        // Blur-up placeholder: same grid cell + `fitClass` as the real
        // image, so it sizes identically and reserves height. `scale-105`
        // bleeds the soft edges past the frame; fades out as the full
        // image fades in. Decorative, so `aria-hidden` + empty alt.
        // eslint-disable-next-line @next/next/no-img-element
        <img
          src={thumbSrc}
          alt=""
          aria-hidden="true"
          decoding="async"
          className={`block ${fitClass} scale-105 transition-opacity duration-300 ease-out motion-reduce:transition-none ${
            loaded ? "opacity-0" : "opacity-100"
          }`}
          // The reader's colour filters ride on the page images, not on a
          // wrapper, so marker overlays and the webtoon footer keep their
          // own colours (night mode would invert them). `blur-xl` moved
          // here because Tailwind's `filter` would otherwise clobber it.
          style={{ filter: `${READER_FILTER_VAR} blur(24px)` }}
        />
      ) : null}
      {/* eslint-disable-next-line @next/next/no-img-element */}
      <img
        ref={imgRef}
        src={effectiveSrc}
        // Retries drop the variant set: the cache-busted `src` must be the
        // URL actually re-fetched, not shadowed by a still-cached variant.
        srcSet={retry > 0 ? undefined : srcSet}
        sizes={srcSet && retry === 0 ? sizes : undefined}
        alt={alt}
        lang={lang}
        loading={loading}
        fetchPriority={fetchPriority}
        decoding="async"
        onLoad={(e) => {
          setSlow(false);
          setStatus("loaded");
          const img = e.currentTarget;
          if (img.naturalWidth > 0) {
            setNaturalAspect(img.naturalWidth / img.naturalHeight);
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
        style={{ filter: READER_FILTER_VAR }}
      />
    </span>
  );
}
