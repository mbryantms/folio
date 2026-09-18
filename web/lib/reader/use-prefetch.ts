import { useEffect, useRef, useSyncExternalStore } from "react";
import type { ViewMode } from "@/lib/reader/detect";
import type { SpreadGroup } from "@/lib/reader/spreads";
import {
  pageVariantUrl,
  selectPageVariantTier,
  withContentVersion,
} from "@/lib/urls";

// How far to warm around the current position. Forward-weighted (most
// turns go forward) but we also warm behind so back-nav is instant.
const AHEAD = 3;
const BEHIND = 2;
// Cap concurrent warm requests so a slow link isn't saturated and the
// visible page (fetchPriority=high) never queues behind prefetches.
const MAX_CONCURRENT = 4;
// Cap retained decoded images so memory stays bounded on long issues.
const MAX_RETAINED = 16;
const MAX_DECODED_PIXELS = 24_000_000; // approximately 96 MB of RGBA

/**
 * Warm upcoming/previous page bytes **and decode them** so the next/prev
 * flip paints instantly.
 *
 * The naive `new Image().src = …` only warms the HTTP byte cache — the
 * decoded frame is dropped, so the real `<img>` still re-decodes on the
 * main thread at display time (visible as a brief blank + the entrance
 * fade). Here we instead call `img.decode()` and **retain** the element,
 * which keeps the browser's decoded copy alive; when the page's real
 * `<img src=…>` mounts it finds a decoded frame and `complete` is true on
 * the first frame, so `PageImage` skips its spinner/fade entirely.
 *
 * Double-page walks by spread group (don't waste a request on the back of
 * a pair we just rendered); single walks by page index. **Webtoon mode
 * skips this entirely** (audit C12): its continuous-scroll layout already
 * mounts a window of real `<img>`s that the browser lazy-loads, so
 * prefetching double-decodes pages whose elements are already in the DOM.
 */
export function useReaderPrefetch(opts: {
  issueId: string;
  totalPages: number;
  currentPage: number;
  currentGroupIdx: number;
  groups: ReadonlyArray<SpreadGroup>;
  viewMode: ViewMode;
  /** Per-page intrinsic widths (FEP-1) — lets warms pick the same
   *  `?w=` tier the rendered `<img>`'s `srcSet` pick resolves to. */
  pageWidths?: ReadonlyArray<number | null>;
  /** False for original-fit / pinch-zoom, which read full-res. */
  variantsEnabled?: boolean;
  /** Archive-content `?v=` stamp — must match the rendered `<img>` URLs
   *  exactly, or the warm cache misses and every page double-fetches. */
  urlVersion?: string | null;
}): void {
  const {
    issueId,
    totalPages,
    currentPage,
    currentGroupIdx,
    groups,
    viewMode,
    pageWidths,
    variantsEnabled = false,
    urlVersion = null,
  } = opts;

  // Retained decoded images, keyed by URL (insertion-ordered for LRU-ish
  // eviction). In-flight set dedupes concurrent warms of the same URL.
  const retained = useRef<Map<string, HTMLImageElement>>(new Map());
  const width = useSyncExternalStore(
    (notify) => {
      window.addEventListener("resize", notify);
      return () => window.removeEventListener("resize", notify);
    },
    () => window.innerWidth,
    () => 1024,
  );

  useEffect(() => {
    retained.current.clear();
  }, [issueId]);

  useEffect(() => {
    // Webtoon relies on the browser lazy-loading its mounted page window
    // (audit C12) — prefetching here just double-decodes. Call the hook
    // unconditionally (rules of hooks); skip the work inside the effect.
    if (viewMode === "webtoon") return;
    const inflight = new Set<string>();
    const images = new Set<HTMLImageElement>();
    let queue: string[] = [];
    let active = 0;
    // FEP-1: mirror the <img> srcSet pick — `sizes` is 100vw (single) /
    // 50vw (double), so the browser targets slot-css-px × dpr and takes
    // the smallest candidate ≥ that. Same formula here keeps warmed URLs
    // byte-identical to what the real element requests.
    const slotCssPx = viewMode === "double" ? width / 2 : width;
    const targetDevicePx = Math.ceil(
      slotCssPx * Math.max(1, window.devicePixelRatio || 1),
    );
    const url = (p: number) => {
      const bare = withContentVersion(
        `/issues/${issueId}/pages/${p}`,
        urlVersion,
      );
      if (!variantsEnabled) return bare;
      const tier = selectPageVariantTier(targetDevicePx, pageWidths?.[p]);
      return tier == null ? bare : pageVariantUrl(bare, tier);
    };

    let cancelled = false;
    const pump = () => {
      while (
        !cancelled &&
        !document.hidden &&
        active < MAX_CONCURRENT &&
        queue.length > 0
      ) {
        const u = queue.shift()!;
        if (retained.current.has(u) || inflight.has(u)) continue;
        inflight.add(u);
        active += 1;
        const img = new Image();
        images.add(img);
        // Hint the browser these are background loads behind the visible page.
        img.fetchPriority = "low";
        img.src = u;
        // decode() resolves once loaded + decoded; retain on success so the
        // decoded frame survives until the page scrolls out of the window.
        img
          .decode()
          .then(() => {
            if (cancelled) return;
            retained.current.set(u, img);
            // Evict oldest beyond the cap (Map preserves insertion order).
            while (
              retained.current.size > MAX_RETAINED ||
              [...retained.current.values()].reduce(
                (n, image) => n + image.naturalWidth * image.naturalHeight,
                0,
              ) > MAX_DECODED_PIXELS
            ) {
              const oldest = retained.current.keys().next().value;
              if (oldest === undefined) break;
              retained.current.delete(oldest);
            }
          })
          .catch(() => {
            /* decode can reject on a load error or an interrupted decode;
               the page will simply load normally when displayed. */
          })
          .finally(() => {
            images.delete(img);
            inflight.delete(u);
            active -= 1;
            pump();
          });
      }
    };

    const want: number[] = [];
    if (viewMode === "double" && groups.length > 0) {
      for (let g = -BEHIND; g <= AHEAD; g += 1) {
        if (g === 0) continue;
        const grp = groups[currentGroupIdx + g];
        if (grp) want.push(...grp);
      }
    } else {
      // single + webtoon: window of page indices around the current one.
      for (let i = -BEHIND; i <= AHEAD; i += 1) {
        if (i === 0) continue;
        const p = currentPage + i;
        if (p >= 0 && p < totalPages) want.push(p);
      }
    }

    // Order forward-first so the most-likely next page warms before the
    // behind pages.
    want.sort(
      (a, b) =>
        Math.sign(b - currentPage) - Math.sign(a - currentPage) ||
        Math.abs(a - currentPage) - Math.abs(b - currentPage),
    );
    queue = want.map(url);
    pump();
    document.addEventListener("visibilitychange", pump);
    return () => {
      cancelled = true;
      queue = [];
      images.forEach((image) => {
        image.src = "";
      });
      document.removeEventListener("visibilitychange", pump);
    };
  }, [
    currentPage,
    currentGroupIdx,
    groups,
    issueId,
    totalPages,
    viewMode,
    urlVersion,
    pageWidths,
    variantsEnabled,
    width,
  ]);
}
