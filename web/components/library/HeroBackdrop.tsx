"use client";

import { useEffect } from "react";

import { rgb, useCoverPalette } from "@/lib/ui/cover-palette";
import { publishPageTint } from "@/lib/ui/page-tint";
import { useVisualEffect } from "@/lib/visual-effects";

/**
 * "Tinted backdrop" effect for the series and issue page headers
 * (folio-flair-1.0). A heavily blurred copy of the cover fills the top of
 * the page, washed with the cover's strongest colour and faded into the
 * page background, so the header takes on the cover's mood without hurting
 * text contrast.
 *
 * Place it as the first child of a `relative isolate` container; it sits
 * behind that container's content (`-z-10`) and bleeds out to the edges of
 * the main content area (it cancels MainShell's `px-4 md:px-8` and
 * `py-6 md:py-8` padding). Renders nothing with the effect off or without a
 * cover.
 */
export function HeroBackdrop({ src }: { src: string | null | undefined }) {
  const on = useVisualEffect("hero_backdrop");
  const palette = useCoverPalette(on ? src : null);

  // Hand the cover colours to the shell (top bar / sidebar / accent,
  // depending on the tint reach — lib/ui/page-tint.ts).
  useEffect(() => {
    if (!on || !src || !palette) return;
    return publishPageTint({
      vibrant: rgb(palette.vibrant),
      average: rgb(palette.average),
      src,
    });
  }, [on, src, palette]);

  if (!on || !src) return null;

  const tint = palette ? rgb(palette.vibrant, 0.55) : "transparent";
  const base = palette ? rgb(palette.average, 0.35) : "transparent";

  return (
    <div
      aria-hidden
      className="pointer-events-none absolute -inset-x-4 -top-6 -z-10 h-[26rem] overflow-hidden sm:h-[30rem] md:-inset-x-8 md:-top-8"
    >
      {/* eslint-disable-next-line @next/next/no-img-element */}
      <img
        src={src}
        alt=""
        decoding="async"
        className="hero-backdrop-img absolute inset-0 h-full w-full scale-125 object-cover"
      />
      <div
        className="absolute inset-0 transition-[background] duration-700"
        style={{
          background: `radial-gradient(120% 90% at 20% 0%, ${tint}, transparent 70%), linear-gradient(${base}, ${base})`,
        }}
      />
      <div className="hero-backdrop-fade absolute inset-0" />
    </div>
  );
}
