"use client";

/**
 * "Smooth card-size changes" (folio-flair-1.0, standard).
 *
 * When a size slider moves, every card on screen glides from its old
 * size and position to the new one instead of the grid or rail snapping.
 * It works for every surface sized by the app-wide cover size
 * (`use-cover-size.ts`: library grid, home rails, saved views, search,
 * series issues, ...) and for the bookmarks' own slider (`useCardSize`):
 *
 * 1. `captureCardLayout()` runs in the size setter, before React
 *    re-renders, and records where each card is.
 * 2. `playCardFlip()` runs from the size hook's layout effect after the
 *    commit (before paint) and animates each card from old to new.
 *
 * Cards are found by `[data-card]` (the card's outer element) and matched
 * by element identity, so the same series in two rails animates
 * correctly. The virtualized grid rebuilds its rows on a column change, so
 * its cells also carry `data-flip-id`, which matches remounted elements by
 * item id. Only cards near the viewport animate, at most 80. Reduced
 * motion skips it.
 */
const SELECTOR = "[data-flip-id], [data-card]:not([data-flip-id] [data-card])";
const MAX_ANIMATED = 80;
const DURATION_MS = 380;

interface Snapshot {
  byEl: Map<Element, DOMRect>;
  byId: Map<string, DOMRect>;
}
let snapshot: Snapshot | null = null;

function reducedMotion(): boolean {
  if (typeof document === "undefined") return true;
  if (document.documentElement.dataset.motion === "reduced") return true;
  return !!window.matchMedia?.("(prefers-reduced-motion: reduce)").matches;
}

function visible(): Array<[HTMLElement, DOMRect]> {
  const vh = window.innerHeight;
  const vw = window.innerWidth;
  const out: Array<[HTMLElement, DOMRect]> = [];
  for (const el of document.querySelectorAll<HTMLElement>(SELECTOR)) {
    const r = el.getBoundingClientRect();
    if (r.width === 0 || r.bottom < -150 || r.top > vh + 150) continue;
    if (r.right < -150 || r.left > vw + 150) continue;
    out.push([el, r]);
  }
  return out;
}

/** Record where every on-screen card is. Call before the size state changes. */
export function captureCardLayout(): void {
  if (typeof window === "undefined" || reducedMotion()) return;
  const byEl = new Map<Element, DOMRect>();
  const byId = new Map<string, DOMRect>();
  for (const [el, r] of visible()) {
    byEl.set(el, r);
    if (el.dataset.flipId) byId.set(el.dataset.flipId, r);
  }
  snapshot = { byEl, byId };
}

/** Animate cards from the captured layout to the current one. Safe to call
 *  from several components after one change: only the first call plays. */
export function playCardFlip(): void {
  const old = snapshot;
  snapshot = null;
  if (!old || typeof window === "undefined") return;
  let n = 0;
  for (const [el, to] of visible()) {
    if (n >= MAX_ANIMATED) break;
    const from =
      old.byEl.get(el) ??
      (el.dataset.flipId ? old.byId.get(el.dataset.flipId) : undefined);
    if (!from) continue;
    const dx = from.left - to.left;
    const dy = from.top - to.top;
    const s = from.width / to.width;
    if (Math.abs(dx) < 1 && Math.abs(dy) < 1 && Math.abs(s - 1) < 0.01)
      continue;
    n++;
    el.animate(
      [
        {
          transformOrigin: "top left",
          transform: `translate(${dx}px, ${dy}px) scale(${s})`,
        },
        { transformOrigin: "top left", transform: "none" },
      ],
      { duration: DURATION_MS, easing: "cubic-bezier(0.2, 0.8, 0.2, 1)" },
    );
  }
}
