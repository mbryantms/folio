"use client";

import * as React from "react";

import {
  Sheet,
  SheetContent,
  SheetHeader,
  SheetTitle,
} from "@/components/folio/sheet";
import { useTouchDevice } from "@/lib/ui/use-coarse-pointer";
import { cn } from "@/lib/utils";

import type { CoverMenuAction } from "./CoverMenuButton";

/**
 * Touch-only companion to {@link CoverMenuButton} + {@link QuickReadOverlay}.
 * Those overlays are hover-revealed (`group-hover:opacity-100`), so on a
 * touch device they're effectively unreachable — there's no hover state to
 * fire. This hook + sheet exposes the same actions via a long-press gesture
 * on the cover wrapper, surfaced as a bottom sheet.
 *
 * Usage: each card calls {@link useCoverLongPressActions} and spreads
 * `wrapperProps` onto the `<div className="relative">` that owns the cover.
 * `sheet` must be rendered somewhere in the card so the bottom sheet has a
 * mount point. On hover-capable devices the hook returns no-op props and a
 * `null` sheet, so the cost on desktop is zero behavior change.
 *
 * Why long-press over "always visible": the cards' covers double as
 * `<Link>` navigation targets. A persistent overlay button would compete
 * for taps with the card's primary navigation. Long-press preserves the
 * single-tap-to-navigate mental model — short tap drills into the detail
 * page, long-press opens the action sheet.
 */
export function useCoverLongPressActions({
  primary,
  actions,
  label,
}: {
  /** Primary action — usually "Read" / "Continue reading". Rendered as
   *  the prominent button at the top of the sheet. Omit for cards
   *  without a primary "read" target. */
  primary?: { label: string; onSelect: () => void };
  /** Same shape as {@link CoverMenuButton} `actions`. */
  actions: CoverMenuAction[];
  /** Sheet title — typically the card's heading (e.g. "Saga #1"). */
  label: string;
}): {
  wrapperProps: TouchHandlerProps;
  sheet: React.ReactNode;
} {
  const isTouch = useTouchDevice();
  const [open, setOpen] = React.useState(false);
  const timer = React.useRef<ReturnType<typeof setTimeout> | null>(null);
  const fired = React.useRef(false);
  const startPos = React.useRef<{ x: number; y: number } | null>(null);
  // Opening-gesture guard. When the long press opens the sheet the finger
  // is still down, and the click the browser synthesizes on lift lands on
  // whichever sheet item now sits under it (the sheet is a portal, so the
  // cover wrapper never sees that click). `blocked` swallows clicks inside
  // the sheet until the opening touch has ended AND a new pointer / touch
  // has gone down on the sheet; `openingEnded` records the first half.
  const guard = React.useRef({ blocked: false, openingEnded: true });
  const detachEndListener = React.useRef<(() => void) | null>(null);

  const clearTimer = React.useCallback(() => {
    if (timer.current !== null) {
      clearTimeout(timer.current);
      timer.current = null;
    }
  }, []);

  const endOpeningGesture = React.useCallback(() => {
    guard.current.openingEnded = true;
    detachEndListener.current?.();
    detachEndListener.current = null;
  }, []);

  // Drop the document listener if the card unmounts mid-gesture.
  React.useEffect(
    () => () => {
      detachEndListener.current?.();
      detachEndListener.current = null;
    },
    [],
  );

  const handlers = React.useMemo<TouchHandlerProps>(() => {
    if (!isTouch) return EMPTY_HANDLERS;
    return {
      "data-cover-long-press": "",
      onTouchStart: (e) => {
        const t = e.touches[0];
        if (!t) return;
        startPos.current = { x: t.clientX, y: t.clientY };
        fired.current = false;
        clearTimer();
        timer.current = setTimeout(() => {
          timer.current = null;
          fired.current = true;
          guard.current = { blocked: true, openingEnded: false };
          // The touchend of the opening finger is dispatched to the cover
          // it started on (and bubbles to the document) even though the
          // sheet now covers it; listen at the document as well as on the
          // wrapper so a parent that stops propagation can't strand the
          // guard in its blocked state.
          detachEndListener.current?.();
          const onEnd = () => endOpeningGesture();
          const opts = { capture: true } as const;
          document.addEventListener("touchend", onEnd, opts);
          document.addEventListener("touchcancel", onEnd, opts);
          detachEndListener.current = () => {
            document.removeEventListener("touchend", onEnd, opts);
            document.removeEventListener("touchcancel", onEnd, opts);
          };
          setOpen(true);
        }, LONG_PRESS_MS);
      },
      onTouchMove: (e) => {
        // Cancel the long-press if the finger drifts beyond MOVE_TOLERANCE_PX.
        // Without this, vertical scrolls would trigger the sheet on every
        // long-glance pause.
        const t = e.touches[0];
        const start = startPos.current;
        if (!t || !start) return;
        const dx = Math.abs(t.clientX - start.x);
        const dy = Math.abs(t.clientY - start.y);
        if (dx > MOVE_TOLERANCE_PX || dy > MOVE_TOLERANCE_PX) {
          clearTimer();
        }
      },
      onTouchEnd: () => {
        clearTimer();
        if (fired.current) endOpeningGesture();
      },
      onTouchCancel: () => {
        clearTimer();
        if (fired.current) endOpeningGesture();
      },
      onContextMenu: (e) => {
        // Android opens its own link / image menu on a long press via
        // `contextmenu`, on top of the sheet. Cancel it only when the
        // event comes from touch (Chrome sends a PointerEvent carrying
        // `pointerType`; older engines don't, and then only while our own
        // touch gesture is in flight). A mouse right-click keeps the
        // browser menu.
        const pointerType = (e.nativeEvent as Partial<PointerEvent>)
          .pointerType;
        const fromTouch =
          pointerType === "touch" ||
          pointerType === "pen" ||
          (!pointerType && (timer.current !== null || fired.current));
        if (fromTouch) e.preventDefault();
      },
      onClick: (e) => {
        // If the long-press fired the sheet, the synthesized click that
        // follows the touchend would still navigate the parent <Link>.
        // Suppress it once per fire so the user lands on the sheet
        // rather than on the detail page underneath.
        if (fired.current) {
          fired.current = false;
          e.preventDefault();
          e.stopPropagation();
        }
      },
    };
  }, [isTouch, clearTimer, endOpeningGesture]);

  const onOpenChange = React.useCallback(
    (next: boolean) => {
      if (!next) {
        guard.current.blocked = false;
        endOpeningGesture();
      }
      setOpen(next);
    },
    [endOpeningGesture],
  );

  // A fresh pointer / touch on the sheet, once the opening finger is up,
  // is a deliberate press: unblock. Keyboard input is always deliberate.
  const arm = () => {
    if (guard.current.openingEnded) guard.current.blocked = false;
  };
  const sheetGuardProps = {
    onPointerDownCapture: arm,
    onTouchStartCapture: arm,
    onKeyDownCapture: () => {
      guard.current.blocked = false;
    },
    onClickCapture: (e: React.MouseEvent) => {
      // `detail === 0` is a click with no pointer behind it (keyboard,
      // assistive tech); the tap synthesized on lift always has detail 1.
      if (guard.current.blocked && e.detail !== 0) {
        e.preventDefault();
        e.stopPropagation();
      }
    },
  };

  const sheet = isTouch ? (
    <Sheet open={open} onOpenChange={onOpenChange}>
      <SheetContent
        side="bottom"
        className="max-h-[80vh] gap-0 overflow-y-auto p-0"
        {...sheetGuardProps}
      >
        <SheetHeader className="px-4 pt-6">
          <SheetTitle className="text-base">{label}</SheetTitle>
        </SheetHeader>
        <ul className="flex flex-col gap-1 p-2" role="menu">
          {primary && (
            <li role="none">
              <button
                type="button"
                role="menuitem"
                className="bg-primary text-primary-foreground hover:bg-primary/90 flex w-full items-center justify-center rounded-md px-4 py-3 text-base font-medium"
                onClick={() => {
                  primary.onSelect();
                  onOpenChange(false);
                }}
              >
                {primary.label}
              </button>
            </li>
          )}
          {actions.map((a, i) => (
            <li key={i} role="none">
              <button
                type="button"
                role="menuitem"
                disabled={a.disabled}
                className={cn(
                  "hover:bg-accent flex w-full items-center justify-start rounded-md px-4 py-3 text-left text-base",
                  "disabled:cursor-not-allowed disabled:opacity-50",
                  a.destructive && "text-destructive",
                )}
                onClick={() => {
                  if (a.disabled) return;
                  a.onSelect();
                  onOpenChange(false);
                }}
              >
                {a.label}
              </button>
            </li>
          ))}
        </ul>
      </SheetContent>
    </Sheet>
  ) : null;

  return { wrapperProps: handlers, sheet };
}

// ───────────────────── internals ─────────────────────

/// 400ms is the same threshold Material/iOS context-menu gestures use, and
/// is short enough not to feel laggy while still being clearly past a tap.
const LONG_PRESS_MS = 400;

/// Drift past this in either axis cancels the long-press. Matches the
/// browser's own tap-vs-scroll threshold.
const MOVE_TOLERANCE_PX = 8;

type TouchHandlerProps = {
  /** Hooks the `[data-cover-long-press]` rule in globals.css that turns
   *  off the native long-press callout on the cover. */
  "data-cover-long-press"?: string;
  onTouchStart?: React.TouchEventHandler<HTMLElement>;
  onTouchMove?: React.TouchEventHandler<HTMLElement>;
  onTouchEnd?: React.TouchEventHandler<HTMLElement>;
  onTouchCancel?: React.TouchEventHandler<HTMLElement>;
  onContextMenu?: React.MouseEventHandler<HTMLElement>;
  onClick?: React.MouseEventHandler<HTMLElement>;
};

const EMPTY_HANDLERS: TouchHandlerProps = {};
