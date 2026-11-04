"use client";

import type { LucideIcon } from "lucide-react";

import { cn } from "@/lib/utils";

import { CoverStack } from "./CoverStack";

/**
 * The top of a list card (Collections, Settings → Views): a full-bleed
 * stage with the list's cover stack centered and large, over a blurred,
 * darkened wash of the front cover, so each card reads by its covers
 * first. Place it as the first child of a `p-4` card; it bleeds to the
 * card's edges with negative margins. Corner controls (kebab, badges) go
 * in `children`, pinned top-right above the stack.
 */
export function CoverStackStage({
  urls,
  Icon,
  children,
  className,
}: {
  urls: readonly string[];
  Icon?: LucideIcon;
  children?: React.ReactNode;
  className?: string;
}) {
  const front = urls[0];
  return (
    <div
      className={cn(
        "relative -mx-4 -mt-4 flex justify-center px-3 pt-7 pb-2",
        className,
      )}
    >
      {/* The wash is clipped to the card's top corners; the stack itself
          is not, so it can still spill on hover. */}
      <div
        aria-hidden
        className="pointer-events-none absolute inset-0 overflow-hidden rounded-t-lg"
      >
        {front ? (
          // eslint-disable-next-line @next/next/no-img-element
          <img
            src={front}
            alt=""
            decoding="async"
            className="absolute inset-0 h-full w-full scale-125 [mask-image:linear-gradient(to_bottom,black_20%,transparent_95%)] object-cover opacity-40 blur-2xl saturate-150"
          />
        ) : null}
      </div>
      <CoverStack
        urls={urls}
        Icon={Icon}
        spillScale={1.08}
        className="relative w-full max-w-[22rem]"
      />
      {children ? (
        <div className="absolute top-2 right-2 z-[5] flex items-center gap-1.5">
          {children}
        </div>
      ) : null}
    </div>
  );
}
