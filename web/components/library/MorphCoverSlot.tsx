"use client";

import { ViewTransition } from "react";

import { Cover } from "@/components/Cover";
import { Skeleton } from "@/components/ui/skeleton";
import { useActiveMorph } from "@/lib/ui/cover-morph";

/**
 * Cover slot of a detail-page loading skeleton. Normally a grey box; while
 * a card cover is morphing into the page, it shows that cover under the
 * shared view-transition name so the flight has somewhere to land before
 * the page itself has loaded (see lib/ui/cover-morph.ts).
 */
export function MorphCoverSlot() {
  const morph = useActiveMorph();
  if (!morph?.src) return <Skeleton className="aspect-[2/3] w-full" />;
  return (
    <ViewTransition name={morph.name} share="morph" default="none">
      <Cover src={morph.src} alt="" />
    </ViewTransition>
  );
}
