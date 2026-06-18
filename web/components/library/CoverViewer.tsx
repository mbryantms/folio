"use client";

/**
 * `<CoverViewer>` — an in-app lightbox for issue covers.
 *
 * Replaces the old "open full resolution in a new tab" (`<a target="_blank">`)
 * link on the cover gallery. In a browser the new tab was fine, but in an
 * installed PWA (`display: standalone`) there is no new tab to open — the
 * link navigated the PWA webview itself onto the raw image-bytes endpoint,
 * stranding the user on a chromeless image with no back button. This keeps
 * the full-resolution view *inside* the app: open, page between covers,
 * close back to the gallery. Radix Dialog gives us the focus trap, scroll
 * lock, and Esc-to-close; the safe-area insets keep the controls clear of
 * the iOS status bar / home indicator.
 */

import * as DialogPrimitive from "@radix-ui/react-dialog";
import { ChevronLeft, ChevronRight, X } from "lucide-react";
import { useCallback, useEffect } from "react";

export type ViewerCover = {
  /** Full-resolution image URL (same URL the thumbnail used, so it's cached). */
  src: string;
  /** Human label — variant label or cover kind. */
  label: string;
  /** Display-formatted provider name, if known. */
  provider?: string | null;
};

export function CoverViewer({
  covers,
  index,
  onIndexChange,
  onClose,
}: {
  /** Ordered list of viewable covers (primary first, then variants). */
  covers: ReadonlyArray<ViewerCover>;
  /** Active index, or `null` when the viewer is closed. */
  index: number | null;
  onIndexChange: (i: number) => void;
  onClose: () => void;
}) {
  const open = index !== null;
  const count = covers.length;

  const go = useCallback(
    (delta: number) => {
      if (index === null || count === 0) return;
      onIndexChange((index + delta + count) % count);
    },
    [index, count, onIndexChange],
  );

  // Left/right arrow keys page between covers while open. (Esc is handled by
  // Radix via `onOpenChange`.)
  useEffect(() => {
    if (!open || count < 2) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "ArrowLeft") {
        e.preventDefault();
        go(-1);
      } else if (e.key === "ArrowRight") {
        e.preventDefault();
        go(1);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, count, go]);

  const current = index !== null ? covers[index] : null;
  if (!current) return null;

  return (
    <DialogPrimitive.Root
      open={open}
      onOpenChange={(o) => {
        if (!o) onClose();
      }}
    >
      <DialogPrimitive.Portal>
        <DialogPrimitive.Overlay className="data-[state=closed]:animate-out data-[state=open]:animate-in data-[state=closed]:fade-out-0 data-[state=open]:fade-in-0 fixed inset-0 z-50 bg-black/90 backdrop-blur-sm" />
        <DialogPrimitive.Content
          aria-describedby={undefined}
          className="data-[state=closed]:animate-out data-[state=open]:animate-in data-[state=closed]:fade-out-0 data-[state=open]:fade-in-0 fixed inset-0 z-50 flex items-center justify-center outline-none"
        >
          {/* Radix requires a labelled title; the visible caption lives in the
              footer, so this one is screen-reader only. */}
          <DialogPrimitive.Title className="sr-only">
            {current.label}
          </DialogPrimitive.Title>

          {/* Backdrop catcher: tapping anywhere that isn't the image (or a
              control) closes the viewer — the expected lightbox gesture. */}
          <button
            type="button"
            aria-label="Close cover viewer"
            onClick={onClose}
            className="absolute inset-0 h-full w-full cursor-default"
            tabIndex={-1}
          />

          {/* Image — fit within the safe viewport, never upscaled past the
              container. `object-contain` preserves the cover aspect. */}
          {/* eslint-disable-next-line @next/next/no-img-element */}
          <img
            src={current.src}
            alt={current.label}
            className="relative max-h-[calc(100dvh_-_var(--safe-top)_-_var(--safe-bottom)_-_4rem)] max-w-[92vw] object-contain select-none"
            draggable={false}
          />

          {/* Close button, top-right, clear of the status bar. */}
          <DialogPrimitive.Close
            className="focus-visible:ring-ring absolute top-[max(0.75rem,var(--safe-top))] right-[max(0.75rem,var(--safe-right))] inline-flex h-10 w-10 items-center justify-center rounded-full bg-black/50 text-white backdrop-blur transition-colors hover:bg-black/70 focus-visible:ring-2 focus-visible:outline-none"
            aria-label="Close"
          >
            <X className="h-5 w-5" />
          </DialogPrimitive.Close>

          {count > 1 && (
            <>
              <button
                type="button"
                onClick={() => go(-1)}
                aria-label="Previous cover"
                className="focus-visible:ring-ring absolute top-1/2 left-[max(0.5rem,var(--safe-left))] inline-flex h-11 w-11 -translate-y-1/2 items-center justify-center rounded-full bg-black/50 text-white backdrop-blur transition-colors hover:bg-black/70 focus-visible:ring-2 focus-visible:outline-none"
              >
                <ChevronLeft className="h-6 w-6" />
              </button>
              <button
                type="button"
                onClick={() => go(1)}
                aria-label="Next cover"
                className="focus-visible:ring-ring absolute top-1/2 right-[max(0.5rem,var(--safe-right))] inline-flex h-11 w-11 -translate-y-1/2 items-center justify-center rounded-full bg-black/50 text-white backdrop-blur transition-colors hover:bg-black/70 focus-visible:ring-2 focus-visible:outline-none"
              >
                <ChevronRight className="h-6 w-6" />
              </button>
            </>
          )}

          {/* Caption + position counter, clear of the home indicator.
              `aria-live` announces the label + "2 / 5" when arrow-paging
              changes the cover, which is otherwise a silent visual update
              (audit E9). */}
          <div
            aria-live="polite"
            aria-atomic="true"
            className="pointer-events-none absolute inset-x-0 bottom-0 flex items-center justify-center gap-2 px-4 pt-8 pb-[max(0.75rem,var(--safe-bottom))] text-center text-sm text-white"
          >
            <span className="inline-flex items-center gap-2 rounded-full bg-black/50 px-3 py-1 backdrop-blur">
              <span className="font-medium capitalize">{current.label}</span>
              {current.provider && (
                <span className="text-white/70">· {current.provider}</span>
              )}
              {count > 1 && (
                <span className="text-white/70">
                  · {index! + 1} / {count}
                </span>
              )}
            </span>
          </div>
        </DialogPrimitive.Content>
      </DialogPrimitive.Portal>
    </DialogPrimitive.Root>
  );
}
