"use client";

import Link from "next/link";
import type { ComponentProps } from "react";

import { markCoverMorph } from "@/lib/ui/cover-morph";

/**
 * A `next/link` that sends a cover along with the navigation (cover morph,
 * lib/ui/cover-morph.ts). For server-rendered surfaces whose cover already
 * carries `morphName` permanently, e.g. a detail page's Read button: the
 * header cover flies into the reader's loading slot, which shows
 * `morphSrc`.
 */
export function MorphLink({
  morphName,
  morphSrc,
  onClick,
  ...props
}: ComponentProps<typeof Link> & {
  morphName: string;
  morphSrc: string | null | undefined;
}) {
  return (
    <Link
      {...props}
      onClick={(e) => {
        onClick?.(e);
        if (e.defaultPrevented) return;
        if (e.metaKey || e.ctrlKey || e.shiftKey || e.altKey || e.button !== 0)
          return;
        markCoverMorph(morphName, morphSrc);
      }}
    />
  );
}
