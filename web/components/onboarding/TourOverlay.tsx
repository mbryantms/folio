"use client";

import * as React from "react";
import { Dialog as DialogPrimitive } from "radix-ui";
import { ArrowLeft, ArrowRight } from "lucide-react";

import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

import { StarterListsPanel } from "./StarterListsPanel";
import { TOUR_STEPS, type TourStep } from "./tour-steps";

type Rect = { top: number; left: number; width: number; height: number };

/** Spotlight padding around the anchor, and the gap to the card. */
const PAD = 6;
const GAP = 14;
/** Phone gutter. */
const EDGE = 16;
const CARD_W = 352;

/** The element a step spotlights. An anchor may narrow itself with
 *  `data-tour-region="<selector>"` (Home's rails point at the first rail
 *  rather than the whole page). */
function anchorElement(step: TourStep): HTMLElement | null {
  if (!step.anchors || typeof document === "undefined") return null;
  let fallback: HTMLElement | null = null;
  for (const anchor of step.anchors) {
    const el = document.querySelector<HTMLElement>(`[data-tour="${anchor}"]`);
    const region = el?.dataset.tourRegion;
    const target = (region && el?.querySelector<HTMLElement>(region)) || el;
    // `display: none` (the other layout's chrome) has no box: skip it.
    if (!target || target.getClientRects().length === 0) continue;
    // Prefer an anchor that's horizontally on screen (the phone nav sheet
    // sits off-canvas); otherwise keep the first so it can scroll in.
    const r = target.getBoundingClientRect();
    if (r.right > 0 && r.left < window.innerWidth) return target;
    fallback ??= target;
  }
  return fallback;
}

/** The anchor's on-screen box, clipped to the viewport; `null` when the
 *  anchor is missing or not visible (the step then shows centered). */
function anchorRect(step: TourStep): Rect | null {
  const el = anchorElement(step);
  if (!el) return null;
  const r = el.getBoundingClientRect();
  if (r.width === 0 || r.height === 0) return null;
  const vw = window.innerWidth;
  const vh = window.innerHeight;
  const top = Math.max(r.top, 0);
  const left = Math.max(r.left, 0);
  const bottom = Math.min(r.bottom, vh);
  const right = Math.min(r.right, vw);
  if (bottom - top < 8 || right - left < 8) return null;
  // Pad, but keep the highlight ring on screen at the viewport edges.
  const t = Math.max(top - PAD, 2);
  const l = Math.max(left - PAD, 2);
  return {
    top: t,
    left: l,
    width: Math.min(right + PAD, vw - 2) - l,
    height: Math.min(bottom + PAD, vh - 2) - t,
  };
}

const sameRect = (a: Rect | null, b: Rect | null) =>
  a === b ||
  (!!a &&
    !!b &&
    Math.abs(a.top - b.top) < 0.5 &&
    Math.abs(a.left - b.left) < 0.5 &&
    Math.abs(a.width - b.width) < 0.5 &&
    Math.abs(a.height - b.height) < 0.5);

/** Track the step's anchor box every frame while the tour is open, so the
 *  spotlight follows layout shifts (rails loading, sidebar animating). */
function useAnchorRect(step: TourStep): Rect | null {
  const [rect, setRect] = React.useState<Rect | null>(null);
  React.useEffect(() => {
    // Bring an off-screen anchor into view (centered, so a sticky top
    // bar never covers it); a visible one stays put.
    const el = anchorElement(step);
    if (el) {
      const r = el.getBoundingClientRect();
      if (r.top < 0 || r.bottom > window.innerHeight) {
        el.scrollIntoView({ block: "center" });
      }
    }
    let raf = 0;
    let last: Rect | null = null;
    const tick = () => {
      const next = anchorRect(step);
      if (!sameRect(next, last)) {
        last = next;
        setRect(next);
      }
      raf = requestAnimationFrame(tick);
    };
    tick();
    return () => cancelAnimationFrame(raf);
  }, [step]);
  return rect;
}

/** Where the card goes: beside the anchor when there's room (sidebar
 *  entries), else below, else above; always inside the viewport. */
function cardPosition(rect: Rect, cardH: number) {
  const vw = window.innerWidth;
  const vh = window.innerHeight;
  const w = Math.min(CARD_W, vw - EDGE * 2);
  const clampX = (x: number) => Math.min(Math.max(x, EDGE), vw - w - EDGE);
  const clampY = (y: number) =>
    Math.min(Math.max(y, EDGE), Math.max(EDGE, vh - cardH - EDGE));
  if (rect.left + rect.width + GAP + w + EDGE <= vw && rect.height < vh * 0.6) {
    return {
      left: rect.left + rect.width + GAP,
      top: clampY(rect.top),
      width: w,
    };
  }
  if (rect.top + rect.height + GAP + cardH + EDGE <= vh) {
    return {
      left: clampX(rect.left),
      top: rect.top + rect.height + GAP,
      width: w,
    };
  }
  if (rect.top - GAP - cardH >= EDGE) {
    return { left: clampX(rect.left), top: rect.top - GAP - cardH, width: w };
  }
  // Tall anchor (Home rails): float the card inside it, bottom-right.
  return {
    left: clampX(rect.left + rect.width - w - GAP),
    top: clampY(rect.top + rect.height - cardH - GAP),
    width: w,
  };
}

/**
 * The guided tour: a modal layer (focus trapped, Esc skips) that dims the
 * page except for the current step's anchor, with the step's card beside
 * it. ← / → move between steps.
 */
export function TourOverlay({
  onFinish,
}: {
  /** Called once, when the reader finishes or skips. */
  onFinish: (how: "done" | "skipped") => void;
}) {
  const [index, setIndex] = React.useState(0);
  const step = TOUR_STEPS[index]!;
  const rect = useAnchorRect(step);
  const last = index === TOUR_STEPS.length - 1;
  const cardRef = React.useRef<HTMLDivElement>(null);
  const [cardH, setCardH] = React.useState(200);
  const nextRef = React.useRef<HTMLButtonElement>(null);

  React.useLayoutEffect(() => {
    const el = cardRef.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setCardH(el.offsetHeight));
    ro.observe(el);
    setCardH(el.offsetHeight);
    return () => ro.disconnect();
  }, [step]);

  // Move focus to the primary action on every step change, so keyboard
  // and screen-reader users land on the new card.
  React.useEffect(() => {
    nextRef.current?.focus({ preventScroll: true });
  }, [index]);

  const go = (delta: number) =>
    setIndex((i) => Math.min(Math.max(i + delta, 0), TOUR_STEPS.length - 1));

  const wide = step.kind === "starter-lists";
  const pos =
    rect && !wide && typeof window !== "undefined"
      ? cardPosition(rect, cardH)
      : null;

  return (
    <DialogPrimitive.Root
      open
      onOpenChange={(open) => {
        if (!open) onFinish("skipped");
      }}
    >
      <DialogPrimitive.Portal>
        <DialogPrimitive.Content
          aria-describedby="tour-step-body"
          className="fixed inset-0 z-[90] outline-none"
          onKeyDown={(e) => {
            if (e.target instanceof HTMLInputElement) return;
            if (e.key === "ArrowRight" && !last) go(1);
            if (e.key === "ArrowLeft") go(-1);
          }}
          // Keep the page under the dimmer inert; only the card is live.
          onPointerDownOutside={(e) => e.preventDefault()}
          // Start on the primary action, not the first button (Skip).
          onOpenAutoFocus={(e) => {
            e.preventDefault();
            nextRef.current?.focus({ preventScroll: true });
          }}
        >
          {/* Dimmer with a hole: a box-shadow around the spotlight box, or
              a plain scrim for centered steps. */}
          {rect && !wide ? (
            <div
              aria-hidden="true"
              className="pointer-events-none fixed rounded-lg transition-all duration-300 ease-out motion-reduce:transition-none"
              style={{
                top: rect.top,
                left: rect.left,
                width: rect.width,
                height: rect.height,
                boxShadow:
                  "0 0 0 2px var(--primary), 0 0 0 9999px rgb(0 0 0 / 0.62)",
              }}
            />
          ) : (
            <div aria-hidden="true" className="fixed inset-0 bg-black/62" />
          )}

          <div
            ref={cardRef}
            className={cn(
              "bg-popover text-popover-foreground border-border fixed flex flex-col rounded-xl border shadow-2xl",
              wide
                ? "top-1/2 left-1/2 max-h-[min(44rem,calc(100dvh-2rem))] w-[min(44rem,calc(100vw-2rem))] -translate-x-1/2 -translate-y-1/2"
                : !pos &&
                    "top-1/2 left-1/2 w-[min(22rem,calc(100vw-2rem))] -translate-x-1/2 -translate-y-1/2",
            )}
            style={
              pos
                ? { top: pos.top, left: pos.left, width: pos.width }
                : undefined
            }
          >
            <div className="space-y-2 p-5 pb-3">
              <p className="text-muted-foreground text-xs tabular-nums">
                Step {index + 1} of {TOUR_STEPS.length}
              </p>
              <DialogPrimitive.Title className="text-lg font-semibold">
                {step.title}
              </DialogPrimitive.Title>
              <p
                id="tour-step-body"
                className="text-muted-foreground text-sm leading-relaxed"
              >
                {step.body}
              </p>
            </div>
            {wide ? (
              <div className="min-h-0 flex-1 overflow-y-auto px-5 pb-2">
                <StarterListsPanel />
              </div>
            ) : null}
            <div className="flex items-center gap-2 border-t p-3">
              <div className="flex gap-1" aria-hidden="true">
                {TOUR_STEPS.map((s, i) => (
                  <span
                    key={s.id}
                    className={cn(
                      "size-1.5 rounded-full",
                      i === index ? "bg-primary" : "bg-muted-foreground/30",
                    )}
                  />
                ))}
              </div>
              <div className="ml-auto flex items-center gap-2">
                {last ? null : (
                  <Button
                    type="button"
                    variant="ghost"
                    size="sm"
                    onClick={() => onFinish("skipped")}
                  >
                    Skip tour
                  </Button>
                )}
                {index > 0 ? (
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    onClick={() => go(-1)}
                    aria-label="Previous step"
                  >
                    <ArrowLeft className="size-4" />
                  </Button>
                ) : null}
                <Button
                  ref={nextRef}
                  type="button"
                  size="sm"
                  onClick={() => (last ? onFinish("done") : go(1))}
                >
                  {last ? (
                    "Done"
                  ) : index === 0 ? (
                    "Show me around"
                  ) : (
                    <>
                      Next <ArrowRight className="ml-1 size-4" />
                    </>
                  )}
                </Button>
              </div>
            </div>
          </div>
        </DialogPrimitive.Content>
      </DialogPrimitive.Portal>
    </DialogPrimitive.Root>
  );
}
