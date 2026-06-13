"use client";

import { X } from "lucide-react";

import type { MarkerMode } from "@/lib/reader/store";

/**
 * Persistent indicator + touch-exit for an active marker mode (audit
 * C7). Entering select-rect/text/image only changed the cursor — useless
 * on touch — while page navigation silently stopped, and the only exit
 * was Esc or an undocumented micro-drag. This floats a pill at the bottom
 * naming the mode, with an explicit ✕ Cancel that resets to idle.
 *
 * Bottom-center (clears the top chrome), `z-40` (above chrome, below the
 * z-50 marker editor / modals).
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
  if (mode === "idle") return null;
  return (
    <div
      className="pointer-events-none fixed inset-x-0 bottom-0 z-40 flex justify-center px-3 pb-[calc(var(--safe-bottom)+1rem)]"
      aria-live="polite"
    >
      <div className="border-border bg-background/95 text-foreground pointer-events-auto flex items-center gap-2 rounded-full border py-1 pr-1 pl-4 shadow-lg backdrop-blur">
        <span className="text-sm font-medium">{MODE_LABEL[mode]}</span>
        <span className="text-muted-foreground hidden text-xs sm:inline">
          Drag to mark · Esc to cancel
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
