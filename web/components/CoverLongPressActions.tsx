"use client";

import * as React from "react";

import {
  Sheet,
  SheetContent,
  SheetHeader,
  SheetTitle,
} from "@/components/ui/sheet";
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
  const isTouch = useIsTouchDevice();
  const [open, setOpen] = React.useState(false);
  const timer = React.useRef<ReturnType<typeof setTimeout> | null>(null);
  const fired = React.useRef(false);
  const startPos = React.useRef<{ x: number; y: number } | null>(null);

  const clearTimer = React.useCallback(() => {
    if (timer.current !== null) {
      clearTimeout(timer.current);
      timer.current = null;
    }
  }, []);

  const handlers = React.useMemo<TouchHandlerProps>(() => {
    if (!isTouch) return EMPTY_HANDLERS;
    return {
      onTouchStart: (e) => {
        const t = e.touches[0];
        if (!t) return;
        startPos.current = { x: t.clientX, y: t.clientY };
        fired.current = false;
        clearTimer();
        timer.current = setTimeout(() => {
          fired.current = true;
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
      onTouchEnd: () => clearTimer(),
      onTouchCancel: () => clearTimer(),
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
  }, [isTouch, clearTimer]);

  const sheet = isTouch ? (
    <Sheet open={open} onOpenChange={setOpen}>
      <SheetContent
        side="bottom"
        className="max-h-[80vh] gap-0 overflow-y-auto p-0"
      >
        <SheetHeader className="px-4 pt-4">
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
                  setOpen(false);
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
                  setOpen(false);
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
  onTouchStart?: React.TouchEventHandler<HTMLElement>;
  onTouchMove?: React.TouchEventHandler<HTMLElement>;
  onTouchEnd?: React.TouchEventHandler<HTMLElement>;
  onTouchCancel?: React.TouchEventHandler<HTMLElement>;
  onClick?: React.MouseEventHandler<HTMLElement>;
};

const EMPTY_HANDLERS: TouchHandlerProps = {};

/**
 * `true` when the primary pointer is coarse and the device cannot hover
 * (phones + touch-only tablets). Uses `useSyncExternalStore` so it stays
 * SSR-safe (returns `false` during prerender — the desktop render path)
 * and picks up runtime changes (rare, but a Surface-style device flipping
 * to touch mode does fire the `change` event).
 */
const TOUCH_QUERY = "(hover: none) and (pointer: coarse)";

function subscribeTouchQuery(callback: () => void): () => void {
  const mq = window.matchMedia(TOUCH_QUERY);
  mq.addEventListener("change", callback);
  return () => mq.removeEventListener("change", callback);
}

function getTouchQuerySnapshot(): boolean {
  return window.matchMedia(TOUCH_QUERY).matches;
}

function getTouchQueryServerSnapshot(): boolean {
  return false;
}

function useIsTouchDevice(): boolean {
  return React.useSyncExternalStore(
    subscribeTouchQuery,
    getTouchQuerySnapshot,
    getTouchQueryServerSnapshot,
  );
}
