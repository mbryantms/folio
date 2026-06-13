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

export function WebtoonEndFooter({
  data,
  isLoading,
  onReadNext,
  exitUrl,
}: {
  data: NextUpView | undefined;
  isLoading: boolean;
  onReadNext: () => void;
  exitUrl: string;
}) {
  const target = data?.target;
  const source = data?.source ?? "none";
  const heading =
    target?.title ??
    (target?.series_name && target?.number
      ? `${target.series_name} #${target.number}`
      : (target?.series_name ?? ""));
  const cblSubtitle =
    source === "cbl" &&
    data?.cbl_position &&
    data?.cbl_total &&
    data?.cbl_list_name
      ? `Issue ${data.cbl_position} of ${data.cbl_total} in ${data.cbl_list_name}`
      : null;

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
          {cblSubtitle ? (
            <p className="text-muted-foreground mt-1 text-xs">{cblSubtitle}</p>
          ) : null}
          <Button onClick={onReadNext} className="mt-4 w-full">
            Read next
          </Button>
          <Button asChild variant="outline" className="mt-2 w-full">
            <Link href={exitUrl}>Back to issue</Link>
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
