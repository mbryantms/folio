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

const LIFT_TRANSITION = ["scale", "rotate", "box-shadow"]
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
