"use client";

import { useState } from "react";

import { cn } from "@/lib/utils";

/** Two-letter monogram: first letters of the first and last words. */
export function monogram(name: string): string {
  const words = name.trim().split(/\s+/).filter(Boolean);
  if (words.length === 0) return "?";
  const first = words[0]?.[0] ?? "";
  const last = words.length > 1 ? (words[words.length - 1]?.[0] ?? "") : "";
  return (first + last).toUpperCase();
}

/** Stable hue for a name, so a person keeps their colour everywhere. */
export function nameHue(name: string): number {
  let h = 2166136261;
  for (const c of name) h = Math.imul(h ^ c.charCodeAt(0), 16777619);
  return (h >>> 0) % 360;
}

/**
 * A creator's (or character's) round portrait: the provider image once a
 * profile fetch stored one, else the coloured monogram every surface used
 * before portraits existed (roadmap WP-9.10). An image that fails to load
 * (a stale provider URL, a CDN that refuses hotlinks) falls back to the
 * monogram too, so a card never shows a broken-image glyph.
 *
 * Decorative: the name is always next to it.
 */
export function CreatorAvatar({
  name,
  src,
  className,
}: {
  name: string;
  src?: string | null;
  /** Size and text size, e.g. `size-12 text-sm`. */
  className?: string;
}) {
  // The failed URL is remembered (not a boolean), so a new `src` gets a
  // fresh attempt without an effect to reset state.
  const [failedSrc, setFailedSrc] = useState<string | null>(null);
  if (src && failedSrc !== src) {
    return (
      // eslint-disable-next-line @next/next/no-img-element
      <img
        src={src}
        alt=""
        loading="lazy"
        referrerPolicy="no-referrer"
        onError={() => setFailedSrc(src)}
        // Portraits are framed on the face: keep the top of a tall image.
        className={cn(
          "bg-muted shrink-0 rounded-full object-cover object-top",
          className,
        )}
      />
    );
  }
  const hue = nameHue(name);
  return (
    <span
      aria-hidden
      data-testid="creator-monogram"
      className={cn(
        "grid shrink-0 place-items-center rounded-full font-semibold text-white",
        className,
      )}
      style={{
        background: `linear-gradient(135deg, oklch(0.62 0.15 ${hue}), oklch(0.48 0.14 ${(hue + 40) % 360}))`,
      }}
    >
      {monogram(name)}
    </span>
  );
}
