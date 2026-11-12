"use client";

import { CSS, type Transform } from "@dnd-kit/utilities";

import { useReducedMotion } from "@/lib/visual-effects";

/**
 * "Springy drag and drop" (folio-flair-1.0, standard), shared by every
 * @dnd-kit sortable:
 *  - `useSpringSort()` — the transition neighbours use when they slide out
 *    of the way: a short overshoot so the list settles rather than sliding
 *    linearly. Pass it as `useSortable({ transition })`. Under reduced
 *    motion it is `undefined` (dnd-kit's plain default).
 *  - `sortableStyle()` — the item's inline style: dnd-kit's transform plus
 *    the lift / settle transitions for the independent `scale`, `rotate`
 *    and `box-shadow` properties.
 *  - `DRAG_LIFT_CLASS` — on the item while it is held (globals.css): it
 *    grows a little, tips, and casts a shadow; taking the class off
 *    springs it back down.
 */
export const SPRING_EASING = "cubic-bezier(0.34, 1.45, 0.64, 1)";

export const SPRING_SORT_TRANSITION = {
  duration: 340,
  easing: SPRING_EASING,
} as const;

export const DRAG_LIFT_CLASS = "drag-lift";

/** Transitions for the lift's own properties, so taking `DRAG_LIFT_CLASS`
 *  off springs an item back down. */
export const LIFT_TRANSITION = ["scale", "rotate", "box-shadow"]
  .map((p) => `${p} 240ms ${SPRING_EASING}`)
  .join(", ");

export function useSpringSort(): typeof SPRING_SORT_TRANSITION | undefined {
  return useReducedMotion() ? undefined : SPRING_SORT_TRANSITION;
}

export function sortableStyle({
  transform,
  transition,
  isDragging,
  translateOnly = false,
}: {
  transform: Transform | null;
  transition: string | undefined;
  isDragging: boolean;
  /** Use `CSS.Translate` (no scale) for items of different sizes. */
  translateOnly?: boolean;
}): React.CSSProperties {
  return {
    transform: (translateOnly ? CSS.Translate : CSS.Transform).toString(
      transform,
    ),
    transition: [transition, LIFT_TRANSITION].filter(Boolean).join(", "),
    zIndex: isDragging ? 20 : undefined,
  };
}

/** Elements inside a draggable item that keep their own click / input
 *  behaviour instead of starting a drag. */
const NO_DRAG =
  'button, [role="button"], [role="menuitem"], [role="checkbox"], [role="switch"], input, select, textarea, label, [contenteditable="true"], [data-no-drag]';

// dnd-kit's SyntheticListenerMap (`Record<string, Function>`).
// eslint-disable-next-line @typescript-eslint/no-unsafe-function-type
type PointerListeners = Record<string, Function>;

/**
 * Grab-anywhere dragging (folio-flair): spread the result on a sortable
 * item so a mouse or pen can pick the whole item up, not just its grip.
 * Presses on the item's own controls (buttons, menus, inputs) keep their
 * normal behaviour; touch keeps the grip handle (a drag from anywhere
 * would fight page scrolling); the click that ends a drag is swallowed so
 * a card that is also a link doesn't navigate when dropped. Keyboard
 * sorting stays on the grip, which carries `attributes` + `onKeyDown`.
 * Pair with a PointerSensor `distance` constraint so plain clicks pass.
 */
export function grabAnywhere(
  listeners: PointerListeners | undefined,
): React.HTMLAttributes<HTMLElement> {
  // The press point is kept on the element, not in this closure: the
  // item re-renders while it is dragged, which would reset a local.
  return {
    onPointerDown: (e) => {
      e.currentTarget.dataset.downAt = `${e.clientX},${e.clientY}`;
      if (e.pointerType === "touch" || e.button !== 0) return;
      if ((e.target as Element).closest(NO_DRAG)) return;
      (
        listeners?.onPointerDown as
          ((e: React.PointerEvent) => void) | undefined
      )?.(e);
    },
    onClickCapture: (e) => {
      const at = e.currentTarget.dataset.downAt;
      delete e.currentTarget.dataset.downAt;
      if (!at) return;
      const [x, y] = at.split(",").map(Number);
      if (Math.hypot(e.clientX - (x ?? 0), e.clientY - (y ?? 0)) > 4) {
        e.preventDefault();
        e.stopPropagation();
      }
    },
    // Native image / link dragging would fight the pointer sensor.
    onDragStart: (e) => e.preventDefault(),
  };
}

/** Where the pointer took hold of the lifted row (from its left edge)
 *  and the row's size, to place the drop label under the pointer. */
export type HeldGrab = { offset: number; width: number; height: number };
export const NO_GRAB: HeldGrab = { offset: 0, width: 0, height: 0 };

/** `.drag-lift` (styles/globals.css): the held row is scaled and tilted
 *  about its centre, so on a wide row the side left of centre hangs
 *  lower than the row's box. */
const LIFT_SCALE = 1.04;
const LIFT_TILT_DEG = 1.25;
/** Space between the lifted row's lowest edge over the label and the
 *  label itself. */
const DROP_LABEL_GAP = 6;

/** The label's left edge (from the row's left) and the top margin that
 *  keeps it clear of the tilted, scaled row above it. */
export function dropLabelPlacement(
  grab: HeldGrab,
  reducedMotion: boolean,
): { left: number; top: number } {
  const left = Math.max(0, grab.offset - 12);
  const grow = (grab.height * (LIFT_SCALE - 1)) / 2;
  const tilt = reducedMotion
    ? 0
    : Math.max(0, grab.width / 2 - left) *
      LIFT_SCALE *
      Math.sin((LIFT_TILT_DEG * Math.PI) / 180);
  return { left, top: Math.ceil(grow + tilt) + DROP_LABEL_GAP };
}
