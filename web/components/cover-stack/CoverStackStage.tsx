"use client";

import type { LucideIcon } from "lucide-react";

import { cn } from "@/lib/utils";

import { CoverStack } from "./CoverStack";

/**
 * The top of a list card (Collections, Settings → Views): the list's
 * cover stack centered and large on the card's own surface, so each card
 * reads by its covers first. (It used to sit on a blurred wash of the
 * front cover; dropped to match the site's plain cards.) Place it as the
 * first child of a `p-4` card; it bleeds to the card's edges with
 * negative margins. Corner controls (kebab, badges) go in `children`,
 * pinned top-right above the stack.
 */
export function CoverStackStage({
  urls,
  Icon,
  children,
  compact = false,
  className,
}: {
  urls: readonly string[];
  /** Half-size stage for dense boards (Settings → Views); the card is
   *  `p-3` instead of `p-4`. */
  compact?: boolean;
  Icon?: LucideIcon;
  children?: React.ReactNode;
  className?: string;
}) {
  return (
    <div
      className={cn(
        "relative flex justify-center",
        compact ? "-mx-3 -mt-3 px-2 pt-3 pb-0" : "-mx-4 -mt-4 px-3 pt-7 pb-2",
        className,
      )}
    >
      <CoverStack
        urls={urls}
        Icon={Icon}
        spillScale={1.08}
        className={cn(
          "relative w-full",
          compact ? "max-w-[8.5rem]" : "max-w-[22rem]",
        )}
      />
      {children ? (
        <div
          className={cn(
            "absolute z-[5] flex items-center gap-1.5",
            compact ? "top-1 right-1" : "top-2 right-2",
          )}
        >
          {children}
        </div>
      ) : null}
    </div>
  );
}
