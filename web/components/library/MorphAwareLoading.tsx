"use client";

import { useActiveMorph } from "@/lib/ui/cover-morph";

/**
 * Loading boundary content that knows about the cover morph. A route group
 * fallback (e.g. `(library)/loading.tsx`, a cover grid) can show first when
 * navigating from Home or a library grid into a detail page; with no cover
 * slot in it, the flying cover would vanish. While a morph is in flight
 * this renders `detail` (the detail-page skeleton, which holds the landing
 * slot) instead of the group's own skeleton.
 */
export function MorphAwareLoading({
  detail,
  children,
}: {
  detail: React.ReactNode;
  children: React.ReactNode;
}) {
  return useActiveMorph() ? detail : children;
}
