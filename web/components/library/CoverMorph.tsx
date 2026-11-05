"use client";

import { ViewTransition } from "react";

/**
 * The shared-name boundary around a morphing cover (lib/ui/cover-morph.ts).
 *
 * Keyed by name on purpose: React only forgets a `<ViewTransition>` name
 * when its fiber unmounts, so changing `name` on a mounted boundary (a card
 * dropping its name when the in-flight mark clears) leaves a stale entry and
 * the next mount of that name warns about a duplicate. A key change makes
 * every name change an unmount + mount, so the bookkeeping stays exact. The
 * cover inside remounts too; `Cover` paints a cached image on its first
 * frame, so the swap is invisible.
 */
export function CoverMorph({
  name,
  landing,
  children,
}: {
  name: string | undefined;
  /** The card's permanent cover name: marks it as a landing spot for the
   *  in-app Back flight (lib/ui/back-flight.ts). A `display: contents`
   *  wrapper, so layout is untouched. */
  landing?: string;
  children: React.ReactNode;
}) {
  const boundary = (
    <ViewTransition key={name ?? ""} name={name} share="morph" default="none">
      {children}
    </ViewTransition>
  );
  if (!landing) return boundary;
  return (
    <div className="contents" data-cover-landing={landing}>
      {boundary}
    </div>
  );
}
