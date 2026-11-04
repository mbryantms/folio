/**
 * Page curl (folio-flair-1.0, effect `page_curl`).
 *
 * A paged-mode page turn drawn as a leaf rotating around its spine edge
 * in 3D (perspective on a fixed overlay, `rotateY` on the leaf), with a
 * gradient that darkens the leaf as it turns away from the light, a sheen
 * that slides across it, and a shadow cast on the page being revealed.
 *
 * Geometry model: a single page reads as the *right* page of an open book
 * in LTR (spine on its left edge) and the *left* page in RTL. A turn has
 * up to two halves:
 *
 *   - **lift**: the outgoing page on the turning side rotates up off the
 *     book until it is edge-on, revealing the new page beneath (the new
 *     page is already rendered by React underneath the overlay);
 *   - **land**: the incoming page on the other side swings down from
 *     edge-on and lands flat, over a copy of the outgoing page on that
 *     side (the "underlay"), which it covers.
 *
 * Spread → spread turns use both halves, like a real book. A single page
 * turning forward only lifts (there is no facing page); turning back it
 * only lands (the previous page swings back over the current one).
 * Mixed turns (a solo cover ↔ a pair) lift the outgoing page only.
 *
 * `planCurl` / `liftSide` are pure (unit-tested); `runPageCurl` builds the
 * overlay and drives it with the Web Animations API.
 */
import type { Direction } from "./detect";

export type CurlSide = "left" | "right";

export interface PageSnap {
  /** Viewport rect of the rendered page image. */
  rect: { left: number; top: number; width: number; height: number };
  /** What to draw: the exact decoded source when known (`currentSrc`),
   *  else the element's `src`/`srcset`/`sizes` so the browser picks the
   *  same cached variant. */
  src: string;
  srcset?: string;
  sizes?: string;
  /** Computed CSS filter of the page (reader colour adjustments). */
  filter?: string;
}

export interface CurlPlan {
  lift: { side: CurlSide; page: PageSnap } | null;
  land: { side: CurlSide; page: PageSnap; under: PageSnap } | null;
}

/** Total duration of a curl turn (ms). */
export const CURL_MS = 520;

/** Which visual side's page lifts first. Forward in LTR turns the right
 *  page over to the left; RTL mirrors it; going back reverses both. */
export function liftSide(direction: Direction, forward: boolean): CurlSide {
  return (direction !== "rtl") === forward ? "right" : "left";
}

const other = (s: CurlSide): CurlSide => (s === "left" ? "right" : "left");

/**
 * Plan a curl from the outgoing pages to the incoming pages (each in
 * visual left-to-right order). Returns null when there is nothing to
 * animate.
 */
export function planCurl({
  direction,
  forward,
  from,
  to,
}: {
  direction: Direction;
  forward: boolean;
  from: readonly PageSnap[];
  to: readonly PageSnap[];
}): CurlPlan | null {
  if (from.length === 0 || to.length === 0) return null;
  const lift = liftSide(direction, forward);
  const land = other(lift);
  const pick = (arr: readonly PageSnap[], side: CurlSide) =>
    side === "left" ? arr[0]! : arr[arr.length - 1]!;

  if (from.length >= 2 && to.length >= 2) {
    return {
      lift: { side: lift, page: pick(from, lift) },
      land: { side: land, page: pick(to, land), under: pick(from, land) },
    };
  }
  if (from.length === 1 && to.length === 1) {
    const pageSide: CurlSide = direction === "rtl" ? "left" : "right";
    return lift === pageSide
      ? { lift: { side: lift, page: from[0]! }, land: null }
      : {
          lift: null,
          land: { side: pageSide, page: to[0]!, under: from[0]! },
        };
  }
  // Solo cover ↔ pair: lift the outgoing page, reveal the new layout.
  return { lift: { side: lift, page: pick(from, lift) }, land: null };
}

/** Rotation (deg) that stands a leaf on `side` up toward the viewer:
 *  a right-hand leaf hinges on its left edge and swings left (negative),
 *  a left-hand leaf hinges on its right edge and swings right. */
export function uprightAngle(side: CurlSide): number {
  return side === "right" ? -90 : 90;
}

/** x of the spine (hinge) edge for a leaf on `side`. */
export function spineX(side: CurlSide, rect: PageSnap["rect"]): number {
  return side === "right" ? rect.left : rect.left + rect.width;
}

// ---------------------------------------------------------------------------
// DOM

function box(el: HTMLElement, r: PageSnap["rect"]) {
  Object.assign(el.style, {
    position: "absolute",
    left: `${r.left}px`,
    top: `${r.top}px`,
    width: `${r.width}px`,
    height: `${r.height}px`,
  });
}

function pageImg(p: PageSnap): HTMLImageElement {
  const img = document.createElement("img");
  img.alt = "";
  img.decoding = "sync";
  if (p.srcset) img.srcset = p.srcset;
  if (p.sizes) img.sizes = p.sizes;
  img.src = p.src;
  Object.assign(img.style, {
    display: "block",
    width: "100%",
    height: "100%",
    filter: p.filter && p.filter !== "none" ? p.filter : "",
  });
  return img;
}

function layer(css: Partial<CSSStyleDeclaration>): HTMLElement {
  const el = document.createElement("i");
  Object.assign(el.style, {
    position: "absolute",
    inset: "0",
    pointerEvents: "none",
    ...css,
  });
  return el;
}

/** "to left" / "to right": from the spine toward the leaf's free edge. */
const awayFromSpine = (side: CurlSide) =>
  side === "right" ? "to right" : "to left";

function leaf(side: CurlSide, page: PageSnap) {
  const el = document.createElement("div");
  box(el, page.rect);
  Object.assign(el.style, {
    transformOrigin: side === "right" ? "0% 50%" : "100% 50%",
    backfaceVisibility: "hidden",
    willChange: "transform",
    boxShadow: "0 0 28px rgb(0 0 0 / 0.35)",
  });
  el.append(pageImg(page));
  const shade = layer({
    background: `linear-gradient(${awayFromSpine(side)}, rgb(0 0 0 / 0.2), rgb(0 0 0 / 0.55))`,
    opacity: "0",
  });
  const sheen = layer({
    background: `linear-gradient(${side === "right" ? 100 : 260}deg, transparent 35%, rgb(255 255 255 / 0.32) 50%, transparent 65%)`,
    backgroundSize: "300% 100%",
    backgroundPosition: "110% 0",
  });
  el.append(shade, sheen);
  return { el, shade, sheen };
}

/** Shadow the turning leaf casts on the page under it, darkest at the
 *  spine. Its width shrinks as the leaf stands up. */
function cast(side: CurlSide, rect: PageSnap["rect"]) {
  const el = document.createElement("div");
  box(el, rect);
  Object.assign(el.style, {
    pointerEvents: "none",
    transformOrigin: side === "right" ? "0% 50%" : "100% 50%",
    background: `linear-gradient(${awayFromSpine(side)}, rgb(0 0 0 / 0.6), rgb(0 0 0 / 0) 80%)`,
    opacity: "0",
  });
  return el;
}

/**
 * Build the curl overlay for `plan` and run it. Returns a function that
 * stops the animation and removes the overlay (idempotent; also called
 * automatically when the animation ends).
 */
export function runPageCurl(
  plan: CurlPlan,
  { duration = CURL_MS }: { duration?: number } = {},
): () => void {
  const lift = plan.lift;
  const land = plan.land;
  const both = !!lift && !!land;
  const hinge = lift
    ? { x: spineX(lift.side, lift.page.rect), r: lift.page.rect }
    : land
      ? { x: spineX(land.side, land.page.rect), r: land.page.rect }
      : null;
  if (!hinge) return () => {};
  const widest = Math.max(
    lift?.page.rect.width ?? 0,
    land?.page.rect.width ?? 0,
  );

  const host = document.createElement("div");
  host.setAttribute("aria-hidden", "true");
  host.dataset.pageCurl = "";
  Object.assign(host.style, {
    position: "fixed",
    inset: "0",
    zIndex: "5",
    pointerEvents: "none",
    overflow: "hidden",
    // Perspective on the container, vanishing point on the spine: the
    // leaf is exactly edge-on at 90° and bulges toward the viewer before.
    perspective: `${Math.max(1600, widest * 4)}px`,
    perspectiveOrigin: `${hinge.x}px ${hinge.r.top + hinge.r.height / 2}px`,
  });

  const anims: Animation[] = [];
  const timing: KeyframeAnimationOptions = {
    duration,
    fill: "both",
    easing: "linear",
  };

  if (land) {
    const under = document.createElement("div");
    box(under, land.under.rect);
    under.append(pageImg(land.under));
    host.append(under);
  }

  if (lift) {
    const sh = cast(lift.side, lift.page.rect);
    const lf = leaf(lift.side, lift.page);
    host.append(sh, lf.el);
    const end = both ? 0.5 : 1;
    const ease = "cubic-bezier(.45,.05,.6,.95)";
    const up = uprightAngle(lift.side);
    anims.push(
      lf.el.animate(
        [
          { transform: "rotateY(0deg)", offset: 0, easing: ease },
          { transform: `rotateY(${up}deg)`, offset: end },
          ...(both ? [{ transform: `rotateY(${up}deg)`, offset: 1 }] : []),
        ],
        timing,
      ),
      lf.shade.animate(
        [
          { opacity: 0, offset: 0, easing: ease },
          { opacity: 0.8, offset: end },
          ...(both ? [{ opacity: 0.8, offset: 1 }] : []),
        ],
        timing,
      ),
      lf.sheen.animate(
        [
          { backgroundPosition: "110% 0", offset: 0 },
          { backgroundPosition: "-10% 0", offset: end },
          ...(both ? [{ backgroundPosition: "-10% 0", offset: 1 }] : []),
        ],
        timing,
      ),
      sh.animate(
        [
          { opacity: 0, transform: "scaleX(1)", offset: 0 },
          { opacity: 0.85, transform: "scaleX(0.6)", offset: end * 0.45 },
          { opacity: 0, transform: "scaleX(0.08)", offset: end },
          ...(both
            ? [{ opacity: 0, transform: "scaleX(0.08)", offset: 1 }]
            : []),
        ],
        timing,
      ),
    );
  }

  if (land) {
    const sh = cast(land.side, land.page.rect);
    const lf = leaf(land.side, land.page);
    host.append(sh, lf.el);
    const start = both ? 0.5 : 0;
    const ease = "cubic-bezier(.2,.65,.3,1)";
    const up = uprightAngle(land.side);
    const hold = (k: Keyframe) => (both ? [{ ...k, offset: 0 }] : []);
    anims.push(
      lf.el.animate(
        [
          ...hold({ transform: `rotateY(${up}deg)` }),
          { transform: `rotateY(${up}deg)`, offset: start, easing: ease },
          { transform: "rotateY(0deg)", offset: 1 },
        ],
        timing,
      ),
      lf.shade.animate(
        [
          ...hold({ opacity: 0.8 }),
          { opacity: 0.8, offset: start, easing: ease },
          { opacity: 0, offset: 1 },
        ],
        timing,
      ),
      lf.sheen.animate(
        [
          ...hold({ backgroundPosition: "-10% 0" }),
          { backgroundPosition: "-10% 0", offset: start },
          { backgroundPosition: "110% 0", offset: 1 },
        ],
        timing,
      ),
      sh.animate(
        [
          ...hold({ opacity: 0, transform: "scaleX(0.08)" }),
          { opacity: 0, transform: "scaleX(0.08)", offset: start },
          {
            opacity: 0.7,
            transform: "scaleX(0.6)",
            offset: start + (1 - start) * 0.6,
          },
          { opacity: 0, transform: "scaleX(1)", offset: 1 },
        ],
        timing,
      ),
    );
  }

  document.body.append(host);

  let done = false;
  const stop = () => {
    if (done) return;
    done = true;
    clearTimeout(safety);
    for (const a of anims) {
      try {
        a.cancel();
      } catch {
        // already finished / detached
      }
    }
    host.remove();
  };
  // Belt and braces: never leave an overlay behind if `finished` never
  // settles (a backgrounded tab can pause animations indefinitely).
  const safety = setTimeout(stop, duration + 400);
  void anims[0]?.finished.then(stop, () => {});
  return stop;
}
