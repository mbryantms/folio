"use client";

import type { DamagedPageView } from "@/lib/api/types";
import { useReaderStore } from "@/lib/reader/store";

/**
 * A small pill at the bottom of the reader while a page Folio kept from a
 * damaged archive is on screen (`issue.damaged_pages`): the reader sees
 * that the broken artwork is the file's, not a loading glitch. Purely
 * informational (no controls, no pointer events), under the chrome
 * (z-20), and at the bottom edge so the top edge stays untouched. The
 * issue page's "damaged page" chip says what to do about it.
 *
 * Self-contained so the reader's first-load bundle carries none of it:
 * the read page mounts it (lazily, see `./lazy`) only for an issue that
 * has damaged pages, and it follows the page from the reader store. In
 * double-page view it treats the page after the current one as on
 * screen too (the right-hand page of the spread).
 */
export function DamagedPageNotice({
  damagedPages,
}: {
  damagedPages: readonly DamagedPageView[];
}) {
  const currentPage = useReaderStore((s) => s.currentPage);
  const viewMode = useReaderStore((s) => s.viewMode);
  const shown =
    viewMode === "double" ? [currentPage, currentPage + 1] : [currentPage];
  const pages = damagedPages.filter((d) => shown.includes(d.page_index));
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
