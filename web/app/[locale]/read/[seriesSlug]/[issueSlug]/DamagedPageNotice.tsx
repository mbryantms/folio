"use client";

import type { DamagedPageView } from "@/lib/api/types";

/**
 * A small pill at the bottom of the reader while a page Folio kept from a
 * damaged archive is on screen (`issue.damaged_pages`): the reader sees
 * that the broken artwork is the file's, not a loading glitch. Purely
 * informational (no controls, no pointer events), under the chrome
 * (z-20), and at the bottom edge so the top edge stays untouched. The
 * issue page's "damaged page" chip says what to do about it.
 */
export function DamagedPageNotice({
  pages,
}: {
  pages: readonly DamagedPageView[];
}) {
  if (pages.length === 0) return null;
  const placeholder = pages.some((p) => p.kind === "placeholder");
  const which = pages.map((p) => p.page_index + 1).join(" and ");
  const text = placeholder
    ? `Page ${which} is damaged in the file: nothing could be recovered.`
    : `Page ${which} is damaged in the file: shown as recovered.`;
  return (
    <div
      className="pointer-events-none fixed inset-x-0 bottom-0 z-20 flex justify-center px-3 pb-[calc(var(--safe-bottom,0px)+0.75rem)]"
      role="status"
      data-testid="damaged-page-notice"
    >
      <p className="border-border bg-background/90 text-foreground rounded-full border px-3 py-1 text-xs shadow-md backdrop-blur">
        {text}
      </p>
    </div>
  );
}
