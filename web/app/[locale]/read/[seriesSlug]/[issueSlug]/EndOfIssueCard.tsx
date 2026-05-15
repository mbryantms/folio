"use client";

/**
 * End-of-issue compact card. Slides in from the page-turn edge (right
 * for LTR, left for RTL) when the user attempts to advance past the
 * last page. Sits as a small floating panel — never blocks the comic
 * the user is still reading.
 *
 * Layout: header (label + close X) → title → cover → primary CTA.
 * Theme-matched via shadcn tokens. Built around a plain fixed-position
 * div rather than Radix Dialog so the reader's window-level keydown
 * listener keeps working through the card.
 */

import { useEffect, useRef } from "react";
import Link from "next/link";
import { X } from "lucide-react";

import { Cover } from "@/components/Cover";
import { Button } from "@/components/ui/button";
import type { Direction } from "@/lib/reader/detect";
import type { NextUpView, OnDeckCard } from "@/lib/api/types";
import { readerUrl } from "@/lib/urls";
import { cn } from "@/lib/utils";

export function EndOfIssueCard({
  open,
  data,
  isLoading,
  direction,
  exitUrl,
  onContinue,
  onDismiss,
}: {
  open: boolean;
  /** Resolver result. `undefined` while the prefetch is in flight; the
   *  card renders a skeleton in that case so a fast reader landing
   *  straight on the last page doesn't show a flicker. */
  data: NextUpView | undefined;
  isLoading: boolean;
  /** Reading direction — controls which edge the card slides in from
   *  so the gesture matches the user's "forward" intent. */
  direction: Direction;
  /** Where the secondary "Exit reader" button goes. Same destination as
   *  the existing reader-exit (the issue detail page). */
  exitUrl: string;
  /** Click handler for the primary CTA. Delegates to the parent so the
   *  keybind and the button fire the same navigation + cache-forwarding
   *  logic. */
  onContinue: () => void;
  /** Closes the card. Doesn't unmount the reader or change the page. */
  onDismiss: () => void;
}) {
  const target = data?.target;
  const source = data?.source ?? "none";
  const cblSubtitle =
    source === "cbl" &&
    data?.cbl_position &&
    data?.cbl_total &&
    data?.cbl_list_name
      ? `Issue ${data.cbl_position} of ${data.cbl_total} in ${data.cbl_list_name}`
      : null;
  const heading =
    target?.title ??
    (target?.series_name && target?.number
      ? `${target.series_name} #${target.number}`
      : (target?.series_name ?? ""));
  const subheading =
    target?.title && target?.series_name && target?.number
      ? `${target.series_name} #${target.number}`
      : null;

  // Focus the primary CTA on open so keyboard users can hit Enter to
  // continue without an extra Tab. requestAnimationFrame so the focus
  // happens after the slide-in animation starts.
  const primaryButtonRef = useRef<HTMLButtonElement>(null);
  const primaryLinkRef = useRef<HTMLAnchorElement>(null);
  useEffect(() => {
    if (!open) return;
    const id = requestAnimationFrame(() => {
      if (source !== "none" && target) {
        primaryButtonRef.current?.focus();
      } else {
        primaryLinkRef.current?.focus();
      }
    });
    return () => cancelAnimationFrame(id);
  }, [open, source, target]);

  const slideFromRight = direction !== "rtl";

  return (
    <div
      role="dialog"
      aria-modal="false"
      aria-label="End of issue"
      aria-hidden={!open}
      className={cn(
        // Floating card — vertically centered, small horizontal gap from
        // the screen edge so the page-turn arrows / chrome remain
        // visible. `max-h-[85vh]` keeps the card from outgrowing the
        // viewport on tiny windows.
        "fixed top-1/2 z-50 flex w-80 max-w-[85vw] -translate-y-1/2 flex-col",
        "bg-background text-foreground rounded-lg border border-border shadow-2xl",
        "max-h-[85vh] overflow-hidden",
        // CSS-only slide animation. `pointer-events-none` while closed
        // keeps the offscreen card from intercepting clicks on the
        // reader chrome below it.
        "transition-transform duration-300 ease-out",
        slideFromRight ? "right-4" : "left-4",
        open
          ? "translate-x-0"
          : slideFromRight
            ? "translate-x-[calc(100%+1.5rem)] pointer-events-none"
            : "-translate-x-[calc(100%+1.5rem)] pointer-events-none",
      )}
    >
      <div className="flex items-start justify-between gap-3 p-4 pb-2">
        <div className="min-w-0 flex-1">
          <p className="text-muted-foreground text-xs font-medium">
            {source === "none" ? "End of the line" : "Next up:"}
          </p>
          {source !== "none" && target ? (
            <h2 className="mt-0.5 text-base font-semibold leading-snug">
              {heading}
            </h2>
          ) : isLoading && !data ? (
            <div className="bg-muted mt-1 h-5 w-40 animate-pulse rounded" />
          ) : (
            <h2 className="mt-0.5 text-base font-semibold leading-snug">
              You&apos;re caught up.
            </h2>
          )}
        </div>
        <button
          type="button"
          onClick={onDismiss}
          aria-label="Close"
          className="text-muted-foreground hover:text-foreground hover:bg-accent flex h-7 w-7 shrink-0 items-center justify-center rounded-md transition-colors"
        >
          <X className="h-4 w-4" />
        </button>
      </div>

      {source !== "none" && target ? (
        <div className="px-4 pb-3">
          <div className="mx-auto w-full">
            <Cover
              src={target.cover_url ?? null}
              alt={heading}
              fallback={heading}
            />
          </div>
          {(subheading || cblSubtitle) && (
            <div className="mt-2">
              {subheading ? (
                <p className="text-muted-foreground text-sm">{subheading}</p>
              ) : null}
              {cblSubtitle ? (
                <p className="text-muted-foreground mt-1 text-xs">
                  {cblSubtitle}
                </p>
              ) : null}
            </div>
          )}
        </div>
      ) : isLoading && !data ? (
        <div className="px-4 pb-3">
          <div
            className="aspect-[2/3] w-full animate-pulse rounded-md bg-neutral-800"
            aria-busy="true"
            aria-label="Loading next issue"
          />
        </div>
      ) : data?.fallback_suggestion ? (
        <div className="px-4 pb-3">
          <p className="text-muted-foreground mb-2 text-sm">
            No next issue in this series, but here&apos;s a suggestion:
          </p>
          <FallbackSuggestionTile
            suggestion={data.fallback_suggestion}
            onNavigate={onDismiss}
          />
        </div>
      ) : (
        <div className="px-4 pb-3">
          <p className="text-muted-foreground text-sm">
            No next issue to suggest right now.
          </p>
        </div>
      )}

      <div className="border-border flex flex-col gap-2 border-t p-4">
        {source !== "none" && target ? (
          <Button
            ref={primaryButtonRef}
            onClick={onContinue}
            className="w-full"
          >
            Read
          </Button>
        ) : (
          <>
            <Button asChild className="w-full">
              <Link
                ref={primaryLinkRef}
                href="/"
                onClick={() => {
                  onDismiss();
                }}
              >
                Browse the library
              </Link>
            </Button>
            <Button asChild variant="outline" className="w-full">
              <Link
                href={exitUrl}
                onClick={() => {
                  onDismiss();
                }}
              >
                Exit reader
              </Link>
            </Button>
          </>
        )}
      </div>
    </div>
  );
}

/**
 * Compact suggestion tile rendered inside the caught-up body when the
 * resolver populated `fallback_suggestion` (D-6). Cover thumb + heading
 * + Read link; the user can ignore it and use the Browse / Exit
 * buttons below, or click through to keep reading.
 */
function FallbackSuggestionTile({
  suggestion,
  onNavigate,
}: {
  suggestion: OnDeckCard;
  onNavigate: () => void;
}) {
  const issue = suggestion.issue;
  // Series_next → keep series-context navigation (no `cbl=`).
  // Cbl_next → forward the saved-view id when we have one so the
  //   reader's next-up resolver stays in the list context.
  const cbl =
    suggestion.kind === "cbl_next"
      ? (suggestion.cbl_saved_view_id ?? null)
      : null;
  const href = readerUrl(issue, { cbl });
  const heading =
    issue.title ??
    (issue.series_name && issue.number
      ? `${issue.series_name} #${issue.number}`
      : (issue.series_name ?? "Untitled"));
  const subheading =
    suggestion.kind === "cbl_next"
      ? `In ${suggestion.cbl_list_name} · entry ${suggestion.position}`
      : suggestion.kind === "series_next"
        ? suggestion.series_name
        : null;
  return (
    <Link
      href={href}
      onClick={onNavigate}
      className={cn(
        "border-border bg-muted/30 hover:bg-muted/60 focus-visible:ring-ring",
        "flex items-center gap-3 rounded-md border p-2 transition-colors",
        "focus-visible:ring-2 focus-visible:outline-none",
      )}
    >
      <div className="w-14 shrink-0">
        <Cover src={issue.cover_url ?? null} alt={heading} fallback={heading} />
      </div>
      <div className="min-w-0 flex-1">
        <div
          className="truncate text-sm font-medium leading-snug"
          title={heading}
        >
          {heading}
        </div>
        {subheading ? (
          <div
            className="text-muted-foreground mt-0.5 truncate text-xs"
            title={subheading}
          >
            {subheading}
          </div>
        ) : null}
      </div>
    </Link>
  );
}
