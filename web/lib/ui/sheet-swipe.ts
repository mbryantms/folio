"use client";

/**
 * Drag-down-to-dismiss for bottom sheets (folio-flair pre-merge polish):
 * the long-press cover sheet and every other `side="bottom"` Sheet, and
 * the dialogs that turn into bottom sheets on phones (globals.css,
 * `[data-slot="dialog-content"]` under 40rem; not the search sheet, which
 * hangs from the top). The grab handle is CSS (`flair-motion.css`).
 *
 * A drag starts from the handle zone or a header at any scroll position,
 * or from anywhere in the sheet while nothing under the finger is
 * scrolled (so an inner list scrolls normally until it is back at the
 * top). The sheet follows the finger; released far or fast enough it is
 * dismissed through Escape (so a caller's `onEscapeKeyDown` guard still
 * applies and only the top-most layer closes), otherwise it springs back.
 * Reduced motion: no spring, it snaps back; dismissing still works.
 *
 * Listeners go on each sheet element as Radix mounts it (a body
 * MutationObserver), never on the document, so page scrolling outside a
 * sheet never waits on a non-passive touch handler.
 */

const SELECTOR =
  '[data-slot="sheet-content"][data-side="bottom"], [data-slot="dialog-content"]:not([data-search-sheet])';
/** Pixels from the sheet's top edge that count as the grab handle. */
const HANDLE_ZONE = 32;
const SLOP = 8;
const DISMISS_FRACTION = 0.25;
const DISMISS_MAX_PX = 140;
const FLING_PX_PER_MS = 0.6;
const SPRING = "translate 480ms cubic-bezier(0.34, 1.56, 0.64, 1)";
const NO_DRAG =
  'input, textarea, select, canvas, [contenteditable="true"], [role="slider"], [aria-roledescription="sortable"], [data-no-sheet-swipe]';

function reduced(): boolean {
  return (
    document.documentElement.getAttribute("data-motion") === "reduced" ||
    (typeof window.matchMedia === "function" &&
      window.matchMedia("(prefers-reduced-motion: reduce)").matches)
  );
}

/** Dialogs are only bottom sheets on phones. */
function isBottomSheet(el: HTMLElement): boolean {
  if (el.dataset.slot === "sheet-content") return true;
  return (
    typeof window.matchMedia === "function" &&
    window.matchMedia("(width < 40rem)").matches
  );
}

/** Something between `target` and `sheet` (inclusive) is scrolled down. */
function scrolledAbove(target: Element, sheet: HTMLElement): boolean {
  for (let n: Element | null = target; n; n = n.parentElement) {
    if (n.scrollTop > 0) return true;
    if (n === sheet) break;
  }
  return false;
}

type Drag = {
  id: number;
  y0: number;
  x0: number;
  dragging: boolean;
  cancelled: boolean;
  samples: Array<{ t: number; y: number }>;
};

/** Wire one sheet element. Returns the cleanup. */
export function attachSheetSwipe(sheet: HTMLElement): () => void {
  let drag: Drag | null = null;
  let suppressClickUntil = 0;
  let resetTimer: ReturnType<typeof setTimeout> | null = null;

  const springBack = () => {
    if (reduced()) {
      sheet.style.transition = "";
      sheet.style.translate = "";
      return;
    }
    sheet.style.transition = SPRING;
    sheet.style.translate = "0px 0px";
    const done = () => {
      sheet.style.transition = "";
      sheet.style.translate = "";
    };
    sheet.addEventListener("transitionend", done, { once: true });
    // A transition that never runs (same value) never ends.
    setTimeout(done, 600);
  };

  const onDown = (e: PointerEvent) => {
    if (e.button !== 0 || !e.isPrimary) return;
    if (sheet.dataset.state !== "open" || !isBottomSheet(sheet)) return;
    const target = e.target as Element | null;
    if (!target || target.closest(NO_DRAG)) return;
    const top = sheet.getBoundingClientRect().top;
    const header = target.closest(
      '[data-slot="sheet-header"], [data-slot="dialog-header"]',
    );
    const fromHandle =
      e.clientY - top < HANDLE_ZONE ||
      (header !== null && sheet.contains(header));
    if (
      !fromHandle &&
      (e.pointerType === "mouse" || scrolledAbove(target, sheet))
    )
      return;
    drag = {
      id: e.pointerId,
      y0: e.clientY,
      x0: e.clientX,
      dragging: false,
      cancelled: false,
      samples: [{ t: e.timeStamp, y: e.clientY }],
    };
  };

  const onMove = (e: PointerEvent) => {
    if (!drag || drag.cancelled || e.pointerId !== drag.id) return;
    const dy = e.clientY - drag.y0;
    const dx = e.clientX - drag.x0;
    if (!drag.dragging) {
      if (Math.abs(dy) < SLOP && Math.abs(dx) < SLOP) return;
      if (dy <= 0 || Math.abs(dx) > Math.abs(dy)) {
        drag = null;
        return;
      }
      drag.dragging = true;
      if (resetTimer) clearTimeout(resetTimer);
      try {
        sheet.setPointerCapture(e.pointerId);
      } catch {
        // The pointer may already be gone (pen lifted); the drag still
        // follows the events that do arrive.
      }
      sheet.style.transition = "none";
      sheet.dataset.sheetDragging = "";
    }
    sheet.style.translate = `0px ${Math.max(0, dy)}px`;
    drag.samples.push({ t: e.timeStamp, y: e.clientY });
    if (drag.samples.length > 6) drag.samples.shift();
  };

  const onUp = (e: PointerEvent) => {
    if (!drag || e.pointerId !== drag.id) return;
    const d = drag;
    drag = null;
    if (!d.dragging) return;
    delete sheet.dataset.sheetDragging;
    suppressClickUntil = performance.now() + 400;
    const offset = Math.max(0, e.clientY - d.y0);
    const first = d.samples[0];
    const last = { t: e.timeStamp, y: e.clientY };
    const v =
      first && last.t > first.t ? (last.y - first.y) / (last.t - first.t) : 0;
    const far =
      offset > Math.min(DISMISS_MAX_PX, sheet.offsetHeight * DISMISS_FRACTION);
    const fast = v > FLING_PX_PER_MS && offset > SLOP * 3;
    if (e.type === "pointerup" && (far || fast)) {
      // Leave the sheet where the finger let go: the exit animation
      // continues from there. Escape goes through Radix's dismiss layer.
      sheet.style.transition = "";
      sheet.dispatchEvent(
        new KeyboardEvent("keydown", {
          key: "Escape",
          code: "Escape",
          bubbles: true,
          cancelable: true,
        }),
      );
      // A guarded sheet (the caller prevented Escape) stays open.
      resetTimer = setTimeout(() => {
        if (sheet.isConnected && sheet.dataset.state === "open") springBack();
      }, 500);
      return;
    }
    springBack();
  };

  const onTouchMove = (e: TouchEvent) => {
    // Hold the page still while the sheet follows the finger (and keep
    // iOS from turning the gesture into a scroll that cancels it).
    if (!drag || drag.cancelled) return;
    const t = e.touches[0];
    if (!t) return;
    if (drag.dragging || t.clientY - drag.y0 > 0) {
      if (e.cancelable) e.preventDefault();
    }
  };

  const onClick = (e: MouseEvent) => {
    if (performance.now() < suppressClickUntil) {
      e.preventDefault();
      e.stopPropagation();
    }
  };

  sheet.addEventListener("pointerdown", onDown);
  sheet.addEventListener("pointermove", onMove);
  sheet.addEventListener("pointerup", onUp);
  sheet.addEventListener("pointercancel", onUp);
  sheet.addEventListener("touchmove", onTouchMove, { passive: false });
  sheet.addEventListener("click", onClick, true);
  return () => {
    if (resetTimer) clearTimeout(resetTimer);
    sheet.removeEventListener("pointerdown", onDown);
    sheet.removeEventListener("pointermove", onMove);
    sheet.removeEventListener("pointerup", onUp);
    sheet.removeEventListener("pointercancel", onUp);
    sheet.removeEventListener("touchmove", onTouchMove);
    sheet.removeEventListener("click", onClick, true);
  };
}

/** Watch the document for bottom sheets and wire each one once. */
export function installSheetSwipe(): () => void {
  const wired = new WeakSet<Element>();
  const scan = (root: ParentNode) => {
    const found: Element[] = [];
    if (root instanceof Element && root.matches(SELECTOR)) found.push(root);
    found.push(...root.querySelectorAll(SELECTOR));
    for (const el of found) {
      if (wired.has(el)) continue;
      wired.add(el);
      // The element is dropped when Radix unmounts it; its listeners go
      // with it.
      attachSheetSwipe(el as HTMLElement);
    }
  };
  scan(document.body);
  const mo = new MutationObserver((records) => {
    for (const r of records)
      for (const n of r.addedNodes) if (n instanceof Element) scan(n);
  });
  mo.observe(document.body, { childList: true, subtree: true });
  return () => mo.disconnect();
}
