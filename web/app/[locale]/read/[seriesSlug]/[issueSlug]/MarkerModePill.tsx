"use client";

import { X } from "lucide-react";

import { primaryPointerIsCoarse } from "@/lib/reader/coarse-pointer";
import { useReaderStore, type MarkerMode } from "@/lib/reader/store";

/**
 * Persistent indicator + touch-exit for an active marker mode (audit
 * C7). Entering select-rect/text/image only changed the cursor — useless
 * on touch — while page navigation silently stopped, and the only exit
 * was Esc or an undocumented micro-drag. This floats a pill at the bottom
 * naming the mode, with an explicit ✕ Cancel that resets to idle.
 *
 * Bottom-center (clears the top chrome), `z-40` (above chrome, below the
 * z-50 marker editor / modals).
 *
 * The hint says how to use the mode: text capture also takes a tap on a
 * detected text box; webtoon on a touch screen needs a press-and-hold
 * before a drag (the page keeps scrolling otherwise). Phones get a short
 * hint without the Esc key they don't have.
 */
const MODE_LABEL: Record<Exclude<MarkerMode, "idle">, string> = {
  "select-rect": "Highlight area",
  "select-text": "Select text",
  "select-image": "Capture image",
};

export function MarkerModePill({
  mode,
  onCancel,
}: {
  mode: MarkerMode;
  onCancel: () => void;
}) {
  const viewMode = useReaderStore((s) => s.viewMode);
  if (mode === "idle") return null;
  const holdToDrag = viewMode === "webtoon" && primaryPointerIsCoarse();
  const text = mode === "select-text";
  // Phone widths: short enough to sit beside the label and Cancel.
  const shortHint = holdToDrag
    ? text
      ? "Tap text or hold + drag"
      : "Hold, then drag"
    : text
      ? "Tap text or drag"
      : "Drag to mark";
  const longHint = `${text ? "Tap a text box or " : ""}${
    holdToDrag
      ? text
        ? "hold, then drag"
        : "Hold, then drag"
      : text
        ? "drag"
        : "Drag"
  } to mark · Esc to cancel`;
  return (
    <div
      className="pointer-events-none fixed inset-x-0 bottom-0 z-40 flex justify-center px-3 pb-[calc(var(--safe-bottom)+1rem)]"
      aria-live="polite"
    >
      <div className="border-border bg-background/95 text-foreground pointer-events-auto flex items-center gap-2 rounded-full border py-1 pr-1 pl-4 shadow-lg backdrop-blur">
        <span className="text-sm font-medium">{MODE_LABEL[mode]}</span>
        <span className="text-muted-foreground min-w-0 text-xs leading-tight sm:hidden">
          {shortHint}
        </span>
        <span className="text-muted-foreground hidden text-xs sm:inline">
          {longHint}
        </span>
        <button
          type="button"
          onClick={onCancel}
          aria-label="Cancel marker mode"
          className="hover:bg-accent inline-flex h-11 items-center gap-1 rounded-full pr-3 pl-2 text-sm font-medium transition-colors"
        >
          <X className="size-4" />
          Cancel
        </button>
      </div>
    </div>
  );
}
