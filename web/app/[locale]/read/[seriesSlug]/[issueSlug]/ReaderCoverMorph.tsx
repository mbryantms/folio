"use client";

import { Loader2 } from "lucide-react";
import { useEffect, useLayoutEffect, useState } from "react";

import { Cover } from "@/components/Cover";
import { CoverMorph } from "@/components/library/CoverMorph";
import {
  holdReaderHandoff,
  peekReaderHandoff,
  releaseReaderHandoff,
  useActiveMorph,
  type CoverMorphFlight,
} from "@/lib/ui/cover-morph";
import { cn } from "@/lib/utils";
import { useReducedMotion } from "@/lib/visual-effects";

/**
 * Cover morph into the reader (folio-flair-1.0, standard).
 *
 * The reader has no header cover to land on, so the tapped cover lands in
 * the loading fallback instead (`ReaderCoverSlot`), centred and sized like
 * a fitted page, and stands in for the first page while the reader's data
 * loads. When the reader mounts, `ReaderCoverHandoff` takes the same cover
 * over at the same spot and under the same view-transition name (so that
 * commit doesn't move it), keeps it until the first page image has
 * decoded, then fades it away over the page.
 */

/** A fitted 2:3 page: full height, or full width on very narrow screens. */
const PAGE_BOX = "w-[min(100vw,calc(100dvh*2/3))] max-w-full";

/** Loading-fallback centre: the in-flight cover, or a spinner. */
export function ReaderCoverSlot() {
  const live = useActiveMorph();
  // Latch: the in-flight mark clears after a few seconds, but a slow
  // reader load should keep showing the cover until it is replaced.
  const [held, setHeld] = useState<CoverMorphFlight | null>(
    live?.src ? live : null,
  );
  if (live?.src && live !== held) setHeld(live);

  useLayoutEffect(() => {
    if (!held) return;
    holdReaderHandoff(held);
    return () => releaseReaderHandoff(held);
  }, [held]);

  if (!held?.src) {
    return (
      <Loader2
        aria-hidden
        className="size-8 animate-spin text-neutral-600 motion-reduce:hidden"
      />
    );
  }
  return (
    <div className={PAGE_BOX}>
      <CoverMorph name={held.name}>
        <Cover src={held.src} alt="" />
      </CoverMorph>
    </div>
  );
}

/** True once a full page image (not a strip thumb) has decoded on screen. */
export function firstPageReady(doc: Document = document): boolean {
  const view = doc.defaultView;
  const vw = view?.innerWidth ?? 0;
  const vh = view?.innerHeight ?? 0;
  for (const img of Array.from(doc.querySelectorAll("img"))) {
    const url = img.currentSrc || img.src;
    if (!/\/pages\/\d+(?:$|[?&#])/.test(url)) continue;
    if (!img.complete || img.naturalWidth === 0) continue;
    const r = img.getBoundingClientRect();
    if (r.width > 0 && r.bottom > 0 && r.top < vh && r.right > 0 && r.left < vw)
      return true;
  }
  return false;
}

/** Longest the cover waits for the first page before giving way anyway. */
const MAX_HOLD_MS = 4000;
const FADE_MS = 220;

/** Mounted with the reader: holds the handed-over cover over the first
 *  page until it has decoded. Renders nothing without a handoff. */
export function ReaderCoverHandoff() {
  // Read once, at the reader's first render — before the commit that
  // unmounts the loading slot (whose cleanup releases the handoff). When
  // the reader's data was already cached the loading slot never mounts,
  // so take the cover straight from the morph in flight instead.
  const live = useActiveMorph();
  const [flight] = useState(
    () => peekReaderHandoff() ?? (live?.src ? live : null),
  );
  const reduced = useReducedMotion();
  const [phase, setPhase] = useState<"hold" | "fade" | "done">(
    flight?.src ? "hold" : "done",
  );

  useEffect(() => {
    if (phase !== "hold") return;
    const start = performance.now();
    let raf = 0;
    const tick = () => {
      if (firstPageReady() || performance.now() - start > MAX_HOLD_MS) {
        setPhase("fade");
        return;
      }
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [phase]);

  useEffect(() => {
    if (phase !== "fade") return;
    const t = setTimeout(() => setPhase("done"), reduced ? 0 : FADE_MS);
    return () => clearTimeout(t);
  }, [phase, reduced]);

  if (!flight?.src || phase === "done") return null;
  return (
    // Above the pages and tap layer (z-10), under the chrome (z-30);
    // click-through so the reader is usable at once.
    <div
      aria-hidden
      data-reader-cover-handoff=""
      className={cn(
        "pointer-events-none fixed inset-0 z-20 grid place-items-center transition-opacity duration-200 ease-out motion-reduce:transition-none",
        phase === "fade" && "opacity-0",
      )}
    >
      <div className={PAGE_BOX}>
        <CoverMorph name={flight.name}>
          <Cover src={flight.src} alt="" />
        </CoverMorph>
      </div>
    </div>
  );
}
