"use client";

import * as React from "react";

import { cn } from "@/lib/utils";

/** `#` then A–Z; values are lowercase to match the server's
 *  case-insensitive `starts_with` (`#` is its own sentinel). Shared
 *  shape with `AtoZJumpRail`. */
export const AZ_BUCKETS: ReadonlyArray<{ value: string; label: string }> = [
  { value: "#", label: "#" },
  ...Array.from({ length: 26 }, (_, i) => {
    const letter = String.fromCharCode(97 + i);
    return { value: letter, label: letter.toUpperCase() };
  }),
];

/**
 * Phone-only (`sm:hidden`) vertical letter scrubber pinned to the right
 * edge between the topbar and the bottom tab bar — the iOS Contacts
 * pattern. Replaces the wrapped A–Z strip, which cost two rows of
 * 24px targets above the grid on phones. Tap a letter to jump, drag
 * to scrub (the letter under the finger is previewed in a bubble and
 * committed on release so the grid refetches once, not 26 times), tap
 * the active letter to clear.
 *
 * Callers add `max-sm:pr-5` to the grid so cards don't sit under it.
 */
export function AtoZScrubber({
  value,
  onSelect,
  className,
}: {
  value: string | null;
  onSelect: (bucket: string | null) => void;
  className?: string;
}) {
  const railRef = React.useRef<HTMLElement | null>(null);
  const [preview, setPreview] = React.useState<string | null>(null);
  const dragging = React.useRef(false);

  const bucketAt = React.useCallback((clientY: number): string => {
    const rect = railRef.current?.getBoundingClientRect();
    if (!rect || rect.height === 0) return AZ_BUCKETS[0]!.value;
    const ratio = (clientY - rect.top) / rect.height;
    const idx = Math.min(
      AZ_BUCKETS.length - 1,
      Math.max(0, Math.floor(ratio * AZ_BUCKETS.length)),
    );
    return AZ_BUCKETS[idx]!.value;
  }, []);

  const onPointerDown = (e: React.PointerEvent<HTMLElement>) => {
    if (e.button !== 0 && e.pointerType === "mouse") return;
    dragging.current = true;
    e.currentTarget.setPointerCapture(e.pointerId);
    setPreview(bucketAt(e.clientY));
  };
  const onPointerMove = (e: React.PointerEvent<HTMLElement>) => {
    if (!dragging.current) return;
    const next = bucketAt(e.clientY);
    setPreview((prev) => (prev === next ? prev : next));
  };
  const onPointerUp = (e: React.PointerEvent<HTMLElement>) => {
    if (!dragging.current) return;
    dragging.current = false;
    const picked = bucketAt(e.clientY);
    setPreview(null);
    onSelect(picked === value ? null : picked);
  };
  const onPointerCancel = () => {
    dragging.current = false;
    setPreview(null);
  };

  const previewLabel = preview
    ? AZ_BUCKETS.find((b) => b.value === preview)?.label
    : null;

  return (
    <>
      <nav
        ref={railRef}
        aria-label="Jump to letter"
        data-az-scrubber=""
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        onPointerCancel={onPointerCancel}
        className={cn(
          // 20px column over the shell's 16px right gutter, so it only eats
          // 4px of content; grids that render the rail add `max-sm:pr-2`
          // on top, which puts the glyphs ~11px off the covers and ~7px
          // off the bezel — the system-index look — instead of glued to
          // the art. The grids keep two
          // columns regardless via the half-width floor in
          // `computeColumnsPerRow` / the CSS `minmax(min(...))` rule.
          "fixed top-(--topbar-h) right-0 bottom-[calc(var(--bottom-tab-h)+var(--safe-bottom))] z-20 flex w-5 touch-none flex-col items-stretch justify-center py-2 select-none sm:hidden",
          className,
        )}
      >
        {AZ_BUCKETS.map((b) => {
          const active = value === b.value;
          const previewing = preview === b.value;
          return (
            <button
              key={b.value}
              type="button"
              aria-pressed={active}
              aria-label={
                b.value === "#"
                  ? "Names starting with a number or symbol"
                  : b.label
              }
              // Pointer input is handled on the rail (so a drag can
              // cross letters); the button only acts on keyboard
              // activation (`detail === 0`) to avoid a double toggle.
              onClick={(e) => {
                if (e.detail !== 0) return;
                onSelect(active ? null : b.value);
              }}
              className={cn(
                "grid min-h-0 flex-1 place-items-center rounded-sm text-[10px] leading-none font-semibold tabular-nums",
                active || previewing ? "text-primary" : "text-muted-foreground",
              )}
            >
              {b.label}
            </button>
          );
        })}
      </nav>
      {previewLabel ? (
        <div
          aria-hidden="true"
          className="bg-foreground text-background fixed top-1/2 right-8 z-30 grid size-14 -translate-y-1/2 place-items-center rounded-full text-2xl font-semibold shadow-lg sm:hidden"
        >
          {previewLabel}
        </div>
      ) : null}
    </>
  );
}
