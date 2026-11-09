"use client";

import { useSyncExternalStore } from "react";

/**
 * In-app Back with a return flight (folio-flair, standard motion).
 *
 * A link into a detail page morphs the clicked card's cover into the page
 * header (lib/ui/cover-morph.ts). Going back can't use the same machinery:
 * React runs back/forward (`popstate`) navigations synchronously so the
 * browser can restore scroll, and a synchronous update never starts a view
 * transition. Pushing the previous URL instead would animate, but it grows
 * history and lands on the route's `loading.tsx` skeleton, which has no
 * card to land on.
 *
 * So the in-app Back button (`BackButton`) does a real `router.back()` and
 * animates by hand: at the click it lays a fixed copy of the header cover
 * over the page; once the previous page is back (and its scroll restored)
 * the copy flies into the card that carries the same cover name
 * (`[data-cover-landing]`, set by `CoverMorph`), which is hidden until the
 * copy arrives. No card on screen: the copy fades out where it is.
 *
 * The browser's own back (and swipe-back gestures, which draw their own
 * animation) is left alone.
 *
 * Also tracks in-app history, so the button only shows when there is a
 * page in this tab's app history to go back to (not after a reload or on
 * a deep link, and not into the reader or sign-in — the breadcrumbs cover
 * those).
 */

// ---------- in-app history ----------

const stack: string[] = [];
let popPending = false;
const subs = new Set<() => void>();
const emit = () => subs.forEach((f) => f());

if (typeof window !== "undefined") {
  window.addEventListener("popstate", () => {
    popPending = true;
  });
}

/** Record the current pathname. Called by `BackFlightLayer` (root layout,
 *  so every route is seen and this stack matches the real history) on every
 *  pathname change. */
export function noteRoute(pathname: string): void {
  const top = stack[stack.length - 1];
  if (top === pathname) {
    popPending = false;
    return;
  }
  if (popPending && stack[stack.length - 2] === pathname) stack.pop();
  else stack.push(pathname);
  popPending = false;
  emit();
}

function subscribe(cb: () => void) {
  subs.add(cb);
  return () => subs.delete(cb);
}

/** Two pages Back doesn't offer, though they are in the history:
 *  - The reader. Its exit pushes the issue page, so the entry behind that page
 *    is the reader itself: Back would go there, and exiting again would land
 *    on the same page with the same Back, with no way past it.
 *  - Sign-in, which pushes the page you were after once you're signed in. */
function isBackTarget(pathname: string): boolean {
  return !pathname.startsWith("/read/") && pathname !== "/sign-in";
}

function canGoBack(): boolean {
  const previous = stack[stack.length - 2];
  return previous !== undefined && isBackTarget(previous);
}

/** True when this tab has an in-app page to go back to. */
export function useCanGoBack(): boolean {
  return useSyncExternalStore(subscribe, canGoBack, () => false);
}

// ---------- the flight ----------

interface Flight {
  name: string;
  clone: HTMLElement;
  from: DOMRect;
  fromPath: string;
  /** Fades the copy out if it hasn't taken off in time. Armed at the click,
   *  so the copy can't outlive the navigation even if nothing ever lands it:
   *  a Back whose pathname doesn't change (`?tab=` entries) never calls
   *  `landBackFlight`. */
  expiry: ReturnType<typeof setTimeout>;
}
/** The flight waiting to land. Cleared once it takes off or starts fading;
 *  from there its animation owns the copy. */
let flight: Flight | null = null;

const EASE = "cubic-bezier(.2,.8,.2,1)";
const WAIT_MS = 700;

function reduced(): boolean {
  return (
    document.documentElement.getAttribute("data-motion") === "reduced" ||
    window.matchMedia("(prefers-reduced-motion: reduce)").matches
  );
}

/** At the Back click: lay a copy of the page's header cover
 *  (`[data-cover-hero]`) over it, ready to fly once the previous page is
 *  back. */
export function startBackFlight(): void {
  dropFlight();
  if (reduced()) return;
  const hero = document.querySelector<HTMLElement>("[data-cover-hero]");
  const name = hero?.dataset.coverHero;
  const img = hero?.querySelector("img");
  if (!hero || !name || !img) return;
  const from = img.getBoundingClientRect();
  if (from.width === 0 || from.bottom < 0 || from.top > innerHeight) return;
  const clone = img.cloneNode(false) as HTMLImageElement;
  clone.removeAttribute("srcset");
  clone.removeAttribute("sizes");
  clone.removeAttribute("loading");
  clone.src = img.currentSrc || img.src;
  clone.alt = "";
  clone.setAttribute("aria-hidden", "true");
  Object.assign(clone.style, {
    position: "fixed",
    left: `${from.left}px`,
    top: `${from.top}px`,
    width: `${from.width}px`,
    height: `${from.height}px`,
    margin: "0",
    objectFit: "cover",
    borderRadius: getComputedStyle(img).borderRadius || "6px",
    zIndex: "60",
    pointerEvents: "none",
    transformOrigin: "0 0",
    willChange: "transform",
    boxShadow: "0 12px 32px rgb(0 0 0 / .35)",
  } satisfies Partial<CSSStyleDeclaration>);
  document.body.appendChild(clone);
  const f: Flight = {
    name,
    clone,
    from,
    fromPath: location.pathname,
    expiry: setTimeout(() => fadeOut(f), WAIT_MS),
  };
  flight = f;
}

function dropFlight() {
  if (!flight) return;
  flight.clone.remove();
  stopWaiting(flight);
}

function stopWaiting(f: Flight) {
  clearTimeout(f.expiry);
  flight = null;
}

function findLanding(name: string): HTMLImageElement | null {
  const boxes = document.querySelectorAll<HTMLElement>(
    `[data-cover-landing="${CSS.escape(name)}"]`,
  );
  for (const box of boxes) {
    const img = box.querySelector("img");
    if (!img) continue;
    const r = img.getBoundingClientRect();
    if (r.width > 0 && r.bottom > 0 && r.top < innerHeight) return img;
  }
  return null;
}

/** After a pathname change: land a pending flight on its card, waiting a
 *  few frames for the page and its scroll restoration to settle. */
export function landBackFlight(pathname: string): void {
  const f = flight;
  if (!f || pathname === f.fromPath) return;
  let last: DOMRect | null = null;
  const tick = () => {
    if (flight !== f) return;
    const target = findLanding(f.name);
    const r = target?.getBoundingClientRect() ?? null;
    const stable = r && last && r.top === last.top && r.left === last.left;
    if (target && r && stable) {
      flyTo(f, target, r);
      return;
    }
    last = r;
    requestAnimationFrame(tick);
  };
  requestAnimationFrame(tick);
}

function flyTo(f: Flight, target: HTMLImageElement, to: DOMRect) {
  stopWaiting(f);
  target.style.visibility = "hidden";
  const { from, clone } = f;
  const dx = to.left - from.left;
  const dy = to.top - from.top;
  const sx = to.width / from.width;
  const sy = to.height / from.height;
  const anim = clone.animate(
    [
      { transform: "none", boxShadow: clone.style.boxShadow },
      {
        transform: `translate(${dx}px, ${dy}px) scale(${sx}, ${sy})`,
        boxShadow: "0 0 0 rgb(0 0 0 / 0)",
      },
    ],
    { duration: 420, easing: EASE, fill: "forwards" },
  );
  const done = () => {
    target.style.visibility = "";
    clone.remove();
  };
  anim.onfinish = done;
  anim.oncancel = done;
}

function fadeOut(f: Flight) {
  stopWaiting(f);
  const anim = f.clone.animate(
    [
      { opacity: 1, transform: "none" },
      { opacity: 0, transform: "scale(.92)" },
    ],
    { duration: 200, easing: "ease-out", fill: "forwards" },
  );
  const done = () => f.clone.remove();
  anim.onfinish = done;
  anim.oncancel = done;
}
