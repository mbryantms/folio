"use client";

import { useEffect, useRef } from "react";
import { Kbd } from "@/components/ui/kbd";
import {
  ChevronLeft,
  ChevronRight,
  Menu,
  MousePointerClick,
  MoveHorizontal,
  ZoomIn,
} from "lucide-react";

import type { Direction } from "@/lib/reader/detect";

/**
 * One-time reader orientation overlay (audit C5). The reader's
 * interaction model — edge tap-zones, swipe-to-turn, double-tap zoom,
 * the keyboard map — was invisible to a first-time reader. This is shown
 * once (gated on {@link hasSeenReaderFirstRun}) the first time anyone
 * opens the reader, names the three tap zones with a direction-aware
 * diagram, and lists the core gestures + shortcuts. Dismissed on the
 * first interaction: a tap anywhere, the "Got it" button, or any key
 * (Escape is consumed here so it doesn't also quit the reader).
 *
 * `z-50` to sit above the chrome (z-30) and mode pill (z-40); it can
 * only appear before any marker editor (also z-50) could be open.
 */
export function ReaderFirstRunOverlay({
  direction,
  onDismiss,
}: {
  direction: Direction;
  onDismiss: () => void;
}) {
  const buttonRef = useRef<HTMLButtonElement>(null);

  // Dismiss on the first key. Capture phase on `window` so it lands
  // before the reader's bubble-phase keymap (use-keymap.ts) — for
  // Escape we consume the event so it doesn't quit the reader too;
  // other keys fall through (ArrowRight both clears this and turns the
  // page). `once` retires the listener after the first dismissal.
  // `onDismiss` is a stable useCallback, so the effect runs once.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopImmediatePropagation();
      }
      onDismiss();
    };
    window.addEventListener("keydown", onKey, { capture: true, once: true });
    return () =>
      window.removeEventListener("keydown", onKey, { capture: true });
  }, [onDismiss]);

  // Pull focus off the page so SR/keyboard users land on the dismiss
  // affordance rather than somewhere behind the scrim.
  useEffect(() => {
    buttonRef.current?.focus();
  }, []);

  const prev = direction === "rtl" ? "Next" : "Back";
  const next = direction === "rtl" ? "Back" : "Next";

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-labelledby="reader-first-run-title"
      onClick={onDismiss}
      className="motion-safe:animate-in motion-safe:fade-in fixed inset-0 z-50 flex items-center justify-center bg-black/70 p-4 backdrop-blur-sm"
    >
      <div
        // Clicks inside the card shouldn't dismiss — only the scrim and
        // the explicit button do. The button calls onDismiss itself.
        onClick={(e) => e.stopPropagation()}
        className="border-border bg-background text-foreground w-full max-w-md rounded-2xl border p-6 shadow-2xl"
      >
        <h2
          id="reader-first-run-title"
          className="text-lg font-semibold tracking-tight"
        >
          Reading, at a glance
        </h2>
        <p className="text-muted-foreground mt-1 text-sm">
          Tap or swipe to get around. Here&apos;s the gist.
        </p>

        {/* Direction-aware tap-zone diagram. */}
        <div
          className="border-border text-muted-foreground mt-5 grid h-24 grid-cols-[1fr_1.2fr_1fr] overflow-hidden rounded-lg border text-xs"
          aria-hidden="true"
        >
          <div className="border-border flex flex-col items-center justify-center gap-1 border-r">
            <ChevronLeft className="size-5" />
            <span>{prev}</span>
          </div>
          <div className="bg-muted/50 flex flex-col items-center justify-center gap-1">
            <Menu className="size-5" />
            <span>Controls</span>
          </div>
          <div className="border-border flex flex-col items-center justify-center gap-1 border-l">
            <ChevronRight className="size-5" />
            <span>{next}</span>
          </div>
        </div>

        {/* Gestures (touch-first). */}
        <ul className="mt-5 space-y-2 text-sm">
          <li className="flex items-center gap-3">
            <MousePointerClick className="text-muted-foreground size-4 shrink-0" />
            <span>
              Tap the left or right edge to turn pages, the center for controls.
            </span>
          </li>
          <li className="flex items-center gap-3">
            <MoveHorizontal className="text-muted-foreground size-4 shrink-0" />
            <span>Swipe left or right to flip between pages.</span>
          </li>
          <li className="flex items-center gap-3">
            <ZoomIn className="text-muted-foreground size-4 shrink-0" />
            <span>Double-tap to zoom; drag to pan while zoomed in.</span>
          </li>
        </ul>

        {/* Keyboard hints — desktop only. */}
        <div className="border-border mt-5 hidden flex-wrap gap-x-4 gap-y-2 border-t pt-4 text-xs sm:flex">
          <Hint keys={["←", "→"]} label="Turn pages" />
          <Hint keys={["d"]} label="View mode" />
          <Hint keys={["f"]} label="Fit" />
          <Hint keys={["t"]} label="Controls" />
          <Hint keys={["?"]} label="All shortcuts" />
        </div>

        <button
          ref={buttonRef}
          type="button"
          onClick={onDismiss}
          className="bg-primary text-primary-foreground hover:bg-primary/90 focus-visible:ring-ring mt-6 inline-flex h-11 w-full items-center justify-center rounded-md text-sm font-medium transition-colors focus-visible:ring-2 focus-visible:outline-none"
        >
          Got it
        </button>
      </div>
    </div>
  );
}

function Hint({ keys, label }: { keys: string[]; label: string }) {
  return (
    <span className="text-muted-foreground inline-flex items-center gap-1.5">
      {keys.map((k) => (
        <Kbd key={k} className="h-auto min-w-5 px-1 py-0.5 text-[0.6875rem]">
          {k}
        </Kbd>
      ))}
      <span>{label}</span>
    </span>
  );
}
