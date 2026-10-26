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
 *
 * Carries the same two Wrap up pieces as the end-of-issue card: the
 * rating strip under the Next up block, and a "Wrap up" disclosure that
 * expands in place (no pane to flip in a scroll flow).
 */

import { useState } from "react";
import Link from "next/link";
import { ChevronDown } from "lucide-react";

import { Cover } from "@/components/Cover";
import { Button } from "@/components/ui/button";
import type { NextUpView } from "@/lib/api/types";
import { nextUpListSubtitle } from "@/lib/reader/list-context";
import type { WrapUpProps } from "@/lib/reader/wrap-up";

import { WrapUpBody, WrapUpStrip } from "./WrapUp";

export function WebtoonEndFooter({
  data,
  isLoading,
  isError = false,
  onRetry,
  onReadNext,
  exitUrl,
  wrapUp,
}: {
  data: NextUpView | undefined;
  isLoading: boolean;
  /** Resolver failed with nothing cached — see `EndOfIssueCard`. */
  isError?: boolean;
  onRetry?: () => void;
  onReadNext: () => void;
  exitUrl: string;
  /** Rating / closing note / markers for the finished issue; omitted →
   *  the footer renders as before. */
  wrapUp?: WrapUpProps;
}) {
  const [wrapOpen, setWrapOpen] = useState(false);
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
      {wrapUp ? (
        <div className="border-border bg-background mt-6 overflow-hidden rounded-lg border text-left">
          <WrapUpStrip wrapUp={wrapUp} onOpen={() => setWrapOpen((o) => !o)} />
          <button
            type="button"
            onClick={() => setWrapOpen((o) => !o)}
            aria-expanded={wrapOpen}
            aria-controls="webtoon-wrap-up"
            className="text-muted-foreground hover:text-foreground border-border flex w-full items-center justify-center gap-1 border-t py-1.5 text-xs transition-colors"
          >
            {wrapOpen ? "Hide" : "Show"} wrap up
            <ChevronDown
              className={`size-3.5 transition-transform ${wrapOpen ? "rotate-180" : ""}`}
              aria-hidden="true"
            />
          </button>
          {wrapOpen ? (
            <div id="webtoon-wrap-up" className="border-border border-t pt-2">
              <WrapUpBody wrapUp={wrapUp} noteId="webtoon-closing-note" />
            </div>
          ) : null}
        </div>
      ) : null}
    </footer>
  );
}
