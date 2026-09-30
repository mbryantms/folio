"use client";

import * as React from "react";
import { useQueries, useQuery, useQueryClient } from "@tanstack/react-query";
import { Loader2, RotateCcw } from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from "@/components/ui/sheet";
import { queryKeys, useIssuePageTextRegions } from "@/lib/api/queries";
import type { TextRegionView } from "@/lib/api/types";
import type { Direction } from "@/lib/reader/detect";
import {
  PAGE_TEXT_CONCURRENCY,
  ocrLangTag,
  readingOrderRegions,
  runInOrderPool,
  usePageTextPanel,
} from "@/lib/reader/page-text";
import { ocrCroppedRegion, type OcrResult } from "./marker-selection";

/**
 * "Page text" panel (WP-4.8, audit AC-3): the OCR'd text of the visible
 * page(s) as plain, ordered text so a screen reader can read the page.
 *
 * Reuses the server OCR surface the marker overlay already drives — the
 * detector's text regions for the page, then one recognizer call per
 * region, in reading order, a couple at a time. Nothing runs until the
 * panel opens (both queries are gated on the panel being mounted, and the
 * Reader only mounts this lazily-imported module on open); results are
 * cached per page + region, so reopening or flipping back is instant.
 *
 * Non-modal on purpose: the reader keymap keeps working while focus is in
 * the panel, so ← / → turn the page and the text follows. Outside clicks
 * don't dismiss it (the page's tap zones would otherwise close it on every
 * turn); Esc, the close button and the `r` shortcut do.
 */
export function PageTextPanel({
  issueId,
  pages,
  direction,
}: {
  issueId: string;
  /** Page indices currently on screen (two in a double-page spread). */
  pages: readonly number[];
  direction: Direction;
}) {
  const open = usePageTextPanel((s) => s.open);
  const setOpen = usePageTextPanel((s) => s.setOpen);
  const label =
    pages.length === 2
      ? `pages ${pages[0]! + 1} and ${pages[1]! + 1}`
      : `page ${(pages[0] ?? 0) + 1}`;
  return (
    <Sheet open={open} onOpenChange={setOpen} modal={false}>
      <SheetContent
        side="right"
        className="flex w-full flex-col gap-0 sm:max-w-md"
        onInteractOutside={(e) => e.preventDefault()}
        // Radix listens for Esc in the document capture phase; stop it
        // there so the reader keymap's `quitReader` doesn't also fire and
        // route the user out of the reader after the panel closes.
        onEscapeKeyDown={(e) => e.stopPropagation()}
      >
        <SheetHeader className="border-border border-b pb-4">
          <SheetTitle>Page text</SheetTitle>
          <SheetDescription>
            Text recognized on {label}, in reading order. Turning the page
            updates it.
          </SheetDescription>
        </SheetHeader>
        {open ? (
          <div className="min-h-0 flex-1 space-y-6 overflow-y-auto py-4">
            {pages.map((page) => (
              <PageTextSection
                key={page}
                issueId={issueId}
                page={page}
                direction={direction}
                showHeading={pages.length > 1}
              />
            ))}
          </div>
        ) : null}
      </SheetContent>
    </Sheet>
  );
}

function regionKey(r: TextRegionView): string {
  return [r.x, r.y, r.w, r.h].map((v) => v.toFixed(2)).join(",");
}

function PageTextSection({
  issueId,
  page,
  direction,
  showHeading,
}: {
  issueId: string;
  page: number;
  direction: Direction;
  showHeading: boolean;
}) {
  const headingId = React.useId();
  const queryClient = useQueryClient();
  const regionsQuery = useIssuePageTextRegions(issueId, page, true);
  const data = regionsQuery.data;
  const ordered = React.useMemo(
    () => (data ? readingOrderRegions(data.regions, direction) : []),
    [data, direction],
  );

  const keys = ordered.map((r) =>
    queryKeys.issuePageRegionText(issueId, page, regionKey(r)),
  );
  const ocrRegion = (region: TextRegionView) => (): Promise<OcrResult | null> =>
    ocrCroppedRegion({
      issueId,
      pageIndex: page,
      region: { ...regionBox(region), shape: "text" },
      naturalSize: { width: data!.page_w, height: data!.page_h },
    });

  // One driver query per page walks the regions in reading order, a
  // couple at a time, filling each region's own cache entry. Aborts its
  // fan-out when the page is turned away (the driver loses its observer).
  useQuery({
    queryKey: queryKeys.issuePageText(issueId, page),
    queryFn: async ({ signal }) => {
      await runInOrderPool(
        ordered,
        PAGE_TEXT_CONCURRENCY,
        (region, i) =>
          queryClient.fetchQuery({
            queryKey: keys[i]!,
            queryFn: ocrRegion(region),
            staleTime: Infinity,
          }),
        signal,
      );
      return true;
    },
    enabled: !!data && ordered.length > 0,
    staleTime: Infinity,
    retry: false,
  });
  // Observe (never fetch) the per-region entries so each block renders
  // as soon as its OCR lands.
  const results = useQueries({
    queries: ordered.map((region, i) => ({
      queryKey: keys[i]!,
      queryFn: ocrRegion(region),
      enabled: false,
      staleTime: Infinity,
    })),
  });

  const settled = results.filter((r) => r.isSuccess || r.isError).length;
  const texts = results
    .map((r, i) => ({ key: keys[i]!.join("|"), result: r.data ?? null }))
    .filter((t): t is { key: string; result: OcrResult } => !!t.result?.text);
  const reading = ordered.length > 0 && settled < ordered.length;

  // Status copy only changes at phase boundaries, so the polite live
  // region announces "finding → reading → done" rather than every region.
  let status: string;
  if (regionsQuery.isError) {
    status = `Couldn't detect text on page ${page + 1}.`;
  } else if (!data) {
    status = `Finding text on page ${page + 1}…`;
  } else if (ordered.length === 0) {
    status = `No text detected on page ${page + 1}.`;
  } else if (reading) {
    status = `Reading text on page ${page + 1}…`;
  } else if (texts.length === 0) {
    status = `Couldn't read any text on page ${page + 1}.`;
  } else {
    status = `Page ${page + 1}: ${texts.length} text ${texts.length === 1 ? "block" : "blocks"}.`;
  }

  return (
    <section aria-labelledby={showHeading ? headingId : undefined}>
      {showHeading ? (
        <h3 id={headingId} className="mb-2 text-sm font-semibold">
          Page {page + 1}
        </h3>
      ) : null}
      <p role="status" className="text-muted-foreground text-sm">
        {status}
      </p>
      {!data && !regionsQuery.isError ? (
        <p className="text-muted-foreground mt-2 flex items-center gap-2 text-xs">
          <Loader2 className="size-3.5 animate-spin" aria-hidden="true" />
          The first scan of a page can take a little while.
        </p>
      ) : null}
      {reading ? (
        <p className="text-muted-foreground mt-2 flex items-center gap-2 text-xs">
          <Loader2 className="size-3.5 animate-spin" aria-hidden="true" />
          {settled} of {ordered.length} regions read
        </p>
      ) : null}
      {regionsQuery.isError ? (
        <Button
          type="button"
          variant="outline"
          size="sm"
          className="mt-3"
          onClick={() => void regionsQuery.refetch()}
        >
          <RotateCcw className="size-3.5" aria-hidden="true" />
          Try again
        </Button>
      ) : null}
      {texts.length > 0 ? (
        <ol
          aria-label={`Text on page ${page + 1}`}
          aria-busy={reading || undefined}
          className="mt-3 space-y-3"
        >
          {texts.map(({ key, result }) => (
            <li
              key={key}
              lang={ocrLangTag(result.lang)}
              className="border-border rounded-md border px-3 py-2 text-sm leading-relaxed whitespace-pre-line"
            >
              {result.text}
            </li>
          ))}
        </ol>
      ) : null}
    </section>
  );
}

function regionBox(r: TextRegionView) {
  return { x: r.x, y: r.y, w: r.w, h: r.h };
}
