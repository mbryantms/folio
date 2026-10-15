"use client";

/**
 * Inline end-of-chapter footer for webtoon mode (audit C2). Webtoon
 * disables swipe + tap-zones, so scrolling to the bottom of a chapter
 * otherwise hit nothing — no end card, no "Up Next". This renders in the
 * scroll flow after the last page so a touch reader reaches the next
 * issue by simply scrolling. Shares the next-up resolver data and the
 * `onReadNext` handler with `EndOfIssueCard` so navigation (incl. CBL
 * context forwarding) stays in one place.
 *
 * Sits at `z-20` so its buttons clear the webtoon chrome-toggle layer
 * (`fixed inset-0 z-10`) that covers the viewport.
 */

import Link from "next/link";

import { Cover } from "@/components/Cover";
import { Button } from "@/components/ui/button";
import type { NextUpView } from "@/lib/api/types";
import { nextUpListSubtitle } from "@/lib/reader/list-context";

export function WebtoonEndFooter({
  data,
  isLoading,
  isError = false,
  onRetry,
  onReadNext,
  exitUrl,
}: {
  data: NextUpView | undefined;
  isLoading: boolean;
  /** Resolver failed with nothing cached — see `EndOfIssueCard`. */
  isError?: boolean;
  onRetry?: () => void;
  onReadNext: () => void;
  exitUrl: string;
}) {
  const target = data?.target;
  const source = data?.source ?? "none";
  const failed = isError && !data;
  const heading =
    target?.title ??
    (target?.series_name && target?.number
      ? `${target.series_name} #${target.number}`
      : (target?.series_name ?? ""));
  const listSubtitle = nextUpListSubtitle(data);

  return (
    <footer className="relative z-20 mx-auto w-full max-w-sm px-6 py-16 text-center">
      {source !== "none" && target ? (
        <>
          <p className="text-muted-foreground text-xs font-medium tracking-wide uppercase">
            Next up
          </p>
          <div className="mx-auto mt-3 w-40">
            <Cover
              src={target.cover_url ?? null}
              alt={heading}
              fallback={heading}
            />
          </div>
          <h2 className="mt-3 text-base leading-snug font-semibold">
            {heading}
          </h2>
          {listSubtitle ? (
            <p className="text-muted-foreground mt-1 text-xs">{listSubtitle}</p>
          ) : null}
          <Button onClick={onReadNext} className="mt-4 w-full">
            Read next
          </Button>
          <Button asChild variant="outline" className="mt-2 w-full">
            <Link href={exitUrl}>Back to issue</Link>
          </Button>
        </>
      ) : failed ? (
        <>
          <p className="text-base leading-snug font-semibold" role="alert">
            Couldn&apos;t load what&apos;s next
          </p>
          <p className="text-muted-foreground mt-1 text-sm">
            The next-issue lookup failed. Your progress is saved.
          </p>
          <Button onClick={onRetry} className="mt-4 w-full" disabled={!onRetry}>
            Retry
          </Button>
          <Button asChild variant="outline" className="mt-2 w-full">
            <Link href={exitUrl}>Exit reader</Link>
          </Button>
        </>
      ) : isLoading && !data ? (
        <div className="space-y-3">
          <div className="bg-muted mx-auto h-4 w-24 animate-pulse rounded" />
          <div className="bg-muted mx-auto aspect-[2/3] w-40 animate-pulse rounded-md" />
        </div>
      ) : (
        <>
          <p className="text-base leading-snug font-semibold">
            You&apos;re caught up.
          </p>
          <p className="text-muted-foreground mt-1 text-sm">
            That was the last issue.
          </p>
          <Button asChild variant="outline" className="mt-4 w-full">
            <Link href={exitUrl}>Exit reader</Link>
          </Button>
        </>
      )}
    </footer>
  );
}
