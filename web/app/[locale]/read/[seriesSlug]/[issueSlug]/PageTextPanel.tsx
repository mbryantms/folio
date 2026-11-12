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
} from "@/components/folio/sheet";
import { useIssuePageTextRegions } from "@/lib/api/queries";
import { queryKeys } from "@/lib/api/query-keys";
import type { Direction } from "@/lib/reader/detect";
import {
  OCR_AUTO_RETRIES,
  ocrRetryDelayMs,
  retryTransientOcr,
  toOcrRequestError,
} from "@/lib/reader/ocr-errors";
import { ocrLangTag, usePageTextPanel } from "@/lib/reader/page-text";
import { resolvePageSize } from "@/lib/reader/page-size";
import { useReaderStore } from "@/lib/reader/store";
import {
  PAGE_TEXT_CONCURRENCY,
  PAGE_TEXT_GC_MS,
  WEBTOON_DETECT_DELAY_MS,
  readingOrderRegions,
  runInOrderPool,
} from "@/lib/reader/text-regions";
import { useSettledFlag } from "@/lib/reader/use-settled-flag";
import { boxTextQueryOptions, type OcrResult } from "./marker-selection";

/** Automatic retries of a transient detection failure (each one is a
 *  detector run against the stricter detection bucket, so fewer than for
 *  a box read). */
const DETECT_AUTO_RETRIES = 2;

/**
 * "Page text" panel (WP-4.8, audit AC-3): the OCR'd text of the visible
 * page(s) as plain, ordered text so a screen reader can read the page.
 *
 * Reuses the server OCR surface the marker overlay already drives — the
 * detector's text regions for the page, then one recognizer call per
 * region, in reading order, a couple at a time. Nothing runs until the
 * panel opens (both queries are gated on the panel being mounted, and the
 * Reader only mounts this lazily-imported module on open); results are
 * cached per page + region for `PAGE_TEXT_GC_MS` after the page is left,
 * so reopening or flipping back during a session is instant. A failed
 * read is never cached as a result: transient ones retry on their own
 * (bounded), and the panel says how many failed and why, with Try again.
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
        size="md"
        side="right"
        className="flex w-full flex-col gap-0"
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
  // Webtoon: ask for a page's text only once it has stayed the current
  // page for a moment, so scrolling with the panel open does not start a
  // detection for every page passed. This section is keyed on the page,
  // so a page scrolled past unmounts before it settles; one that already
  // started is aborted when it loses its last observer.
  const viewMode = useReaderStore((s) => s.viewMode);
  const active = useSettledFlag(
    true,
    viewMode === "webtoon" ? WEBTOON_DETECT_DELAY_MS : 0,
  );
  const regionsQuery = useIssuePageTextRegions(issueId, page, active, {
    gcTime: PAGE_TEXT_GC_MS,
    retry: retryTransientOcr(DETECT_AUTO_RETRIES),
    retryDelay: ocrRetryDelayMs,
  });
  const data = regionsQuery.data;
  // The same boxes, in the same order, that text capture draws and taps
  // (`text-regions.ts`), against the detection's own page size.
  const size = resolvePageSize({
    detected: data ? { width: data.page_w, height: data.page_h } : null,
  });
  const hasSize = size !== null;
  const ordered = React.useMemo(
    () => (data && hasSize ? readingOrderRegions(data.regions, direction) : []),
    [data, hasSize, direction],
  );
  // Transient failures (busy, rate-limited, timed out) retry on their
  // own with a bounded backoff that honours Retry-After; a failure is an
  // error, never a cached "no text".
  const boxQuery = (i: number) => ({
    ...boxTextQueryOptions(issueId, page, ordered[i]!, size!),
    retry: retryTransientOcr(OCR_AUTO_RETRIES),
    retryDelay: ocrRetryDelayMs,
  });

  // One driver query per page walks the boxes in reading order, a
  // couple at a time, filling each box's own cache entry. Aborts its
  // fan-out when the page is turned away (the driver loses its observer).
  useQuery({
    queryKey: queryKeys.issuePageText(issueId, page),
    queryFn: async ({ signal }) => {
      await runInOrderPool(
        ordered,
        PAGE_TEXT_CONCURRENCY,
        (_box, i) => queryClient.fetchQuery(boxQuery(i)),
        signal,
      );
      return true;
    },
    enabled: !!size && ordered.length > 0,
    staleTime: Infinity,
    gcTime: PAGE_TEXT_GC_MS,
    retry: false,
  });
  // Observe (never fetch) the per-box entries so each block renders as
  // soon as its OCR lands.
  const results = useQueries({
    queries: ordered.map((_box, i) => ({
      ...boxQuery(i),
      enabled: false,
    })),
  });

  const failedIdx = results
    .map((r, i) => (r.isError && !r.isFetching ? i : -1))
    .filter((i) => i >= 0);
  const settled = results.filter(
    (r) => r.isSuccess || (r.isError && !r.isFetching),
  ).length;
  const texts = results
    .map((r, i) => ({
      key: boxQuery(i).queryKey.join("|"),
      result: r.data ?? null,
    }))
    .filter((t): t is { key: string; result: OcrResult } => !!t.result?.text);
  const reading = ordered.length > 0 && settled < ordered.length;
  const failures = failedIdx.map((i) => toOcrRequestError(results[i]!.error));
  const canRetryFailed = failures.some((f) => f.transient);
  const [retrying, setRetrying] = React.useState(false);
  const retryFailed = async () => {
    setRetrying(true);
    try {
      await runInOrderPool(failedIdx, PAGE_TEXT_CONCURRENCY, (i) =>
        queryClient.fetchQuery(boxQuery(i)),
      );
    } finally {
      setRetrying(false);
    }
  };

  // Once every box has settled, hand the first block (reading order) to
  // the panel store so the page image's `alt` can quote its opening line
  // (audit N-R4). A page with no readable text clears the slot.
  const setFirstText = usePageTextPanel((s) => s.setFirstText);
  const firstText = texts[0]?.result.text ?? null;
  const firstLang = texts[0]?.result.lang ?? null;
  const settledAll = !!data && !reading;
  React.useEffect(() => {
    if (!settledAll) return;
    setFirstText(issueId, page, firstText, firstLang);
  }, [settledAll, firstText, firstLang, issueId, page, setFirstText]);

  // Status copy only changes at phase boundaries, so the polite live
  // region announces "finding → reading → done" rather than every box.
  const detectFailure = regionsQuery.isError
    ? toOcrRequestError(regionsQuery.error)
    : null;
  let status: string;
  if (detectFailure) {
    status = `Couldn't find the text on page ${page + 1}. ${detectFailure.message}`;
  } else if (!data) {
    status = `Finding text on page ${page + 1}…`;
  } else if (ordered.length === 0) {
    status = `No text detected on page ${page + 1}.`;
  } else if (reading) {
    status = `Reading text on page ${page + 1}…`;
  } else if (failures.length === ordered.length) {
    status = `Couldn't read the text on page ${page + 1}. ${failures[0]!.message}`;
  } else if (texts.length === 0 && failures.length === 0) {
    status = `No readable text found on page ${page + 1}.`;
  } else {
    status = `Page ${page + 1}: ${texts.length} text ${texts.length === 1 ? "block" : "blocks"}.`;
    if (failures.length > 0) {
      status += ` ${failures.length} of ${ordered.length} ${ordered.length === 1 ? "region" : "regions"} couldn't be read. ${failures[0]!.message}`;
    }
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
      {!data && !detectFailure ? (
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
      {detectFailure?.transient ? (
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
      {!reading && canRetryFailed ? (
        <Button
          type="button"
          variant="outline"
          size="sm"
          className="mt-3"
          disabled={retrying}
          onClick={() => void retryFailed()}
        >
          <RotateCcw className="size-3.5" aria-hidden="true" />
          {failures.length === 1
            ? "Try the failed region again"
            : `Try the ${failures.length} failed regions again`}
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
