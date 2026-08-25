/**
 * iOS/iPadOS home-screen web apps (standalone PWAs) have a WebKit
 * defect in the pointer-event path: after an in-app navigation the
 * standalone web-app process can stop delivering the captured
 * `pointermove`/`pointerup` stream that follows `pointerdown`, so
 * every touch drag reads as a motionless tap. The wedge is
 * process-wide — it survives further route changes and only
 * force-quitting the app clears it — and it never reproduces in
 * Safari proper, whose pointer-capture plumbing is separate from the
 * standalone shell's. (Same defect family as WebKit's long-standing
 * installed-PWA pointer-capture bugs; see e.g.
 * https://developer.apple.com/forums/thread/690691 and
 * openseadragon/openseadragon#1962.)
 *
 * Touch-event delivery is unaffected, so every touch-drag surface in
 * the reader — the swipe/pan gesture (`use-swipe.ts`) and the marker
 * overlay's region-select drag (`MarkerOverlay.tsx`) — binds through
 * raw touch events when this predicate is true. Deliberately gated on
 * a coarse primary pointer (phones/tablets — every device that can
 * hit the standalone bug, and where finger drag is the input that
 * matters) rather than mere touch support: binding only touch
 * handlers on any touch-capable device would break mouse drags on
 * touchscreen laptops, so fine-pointer devices keep the pointer-event
 * path.
 */
export function primaryPointerIsCoarse(): boolean {
  return (
    typeof window !== "undefined" &&
    window.matchMedia("(pointer: coarse)").matches
  );
}
