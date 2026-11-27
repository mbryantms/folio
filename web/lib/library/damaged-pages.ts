import type { DamagedPageView } from "@/lib/api/types";

/**
 * One line describing a damaged page, numbered as the reader shows pages
 * (1-based). Shared by the issue-page badge and the reader notice.
 */
export function damagedPageLine(p: DamagedPageView): string {
  const n = p.page_index + 1;
  return p.kind === "placeholder"
    ? `Page ${n}: nothing could be recovered, so a placeholder stands in for it.`
    : `Page ${n}: partly recovered; some of the image may be wrong or missing.`;
}

/** The damaged-page record for 0-based `pageIndex`, if any. */
export function damagedPageAt(
  pages: readonly DamagedPageView[] | undefined,
  pageIndex: number,
): DamagedPageView | undefined {
  return pages?.find((p) => p.page_index === pageIndex);
}
