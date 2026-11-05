"use client";

import { CoverSkeleton } from "@/components/CoverSkeleton";
import { Cover } from "@/components/Cover";
import { CoverMorph } from "@/components/library/CoverMorph";
import { useActiveMorph } from "@/lib/ui/cover-morph";

/**
 * Cover slot of a detail-page loading skeleton. Normally a grey box; while
 * a card cover is morphing into the page, it shows that cover under the
 * shared view-transition name so the flight has somewhere to land before
 * the page itself has loaded (see lib/ui/cover-morph.ts).
 */
export function MorphCoverSlot() {
  const morph = useActiveMorph();
  if (!morph?.src) return <CoverSkeleton />;
  return (
    <CoverMorph name={morph.name}>
      <Cover src={morph.src} alt="" />
    </CoverMorph>
  );
}
