"use client";

/**
 * End-of-issue compact card. Slides in from the page-turn edge (right
 * for LTR, left for RTL) when the user attempts to advance past the
 * last page. Sits as a small floating panel — never blocks the comic
 * the user is still reading.
 *
 * Two panes inside one 320px panel:
 *
 *  - **Up Next** (default) — header (label + close X) → title → cover
 *    → primary CTA → issue-page links, unchanged, plus one strip at the
 *    very bottom: inline stars + a "Wrap up" button (`<WrapUpStrip>`).
 *    Focus lands on Read, so Enter still means "keep reading".
 *  - **Wrap up** — back arrow + the finished issue's name, then
 *    rating / closing note / markers-in-this-issue (`<WrapUpBody>`),
 *    with Read still at the bottom so continuing is never more than
 *    one tap away. Opens only on demand; never auto-focuses the note.
 *
 * While the card is open, `1`–`5` rate the finished issue (a second
 * press on the current value clears). Theme-matched via shadcn tokens.
 * Built around a plain fixed-position div rather than Radix Dialog so
 * the reader's window-level keydown listener keeps working through the
 * card.
 */

import { useEffect, useRef, useState } from "react";
import Link from "next/link";
import { ChevronLeft, X } from "lucide-react";

import { Cover } from "@/components/Cover";
import { Button } from "@/components/ui/button";
import type { Direction } from "@/lib/reader/detect";
import { shouldSkipHotkey } from "@/lib/reader/keybinds";
import type { NextUpView, OnDeckCard } from "@/lib/api/types";
import { nextUpListSubtitle } from "@/lib/reader/list-context";
import { ratingFromKey, type WrapUpProps } from "@/lib/reader/wrap-up";
import { issueUrl, readerUrl } from "@/lib/urls";
import { cn } from "@/lib/utils";

import { WrapUpBody, WrapUpStrip } from "./WrapUp";

export function EndOfIssueCard({
  open,
  data,
  isLoading,
  isError = false,
  onRetry,
  direction,
  exitUrl,
  onContinue,
  onDismiss,
  wrapUp,
}: {
  open: boolean;
  /** Resolver result. `undefined` while the prefetch is in flight; the
   *  card renders a skeleton in that case so a fast reader landing
   *  straight on the last page doesn't show a flicker. */
  data: NextUpView | undefined;
  isLoading: boolean;
  /** The resolver request failed and there is no data to show. Renders
   *  "Couldn't load what's next" + Retry instead of claiming the user
   *  is caught up (post-audit T2.6). */
  isError?: boolean;
  /** Re-runs the resolver query. */
  onRetry?: () => void;
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
  /** Rating / closing note / markers for the issue just finished.
   *  Omitted (tests, embeds) → the card renders exactly as before. */
  wrapUp?: WrapUpProps;
}) {
  const target = data?.target;
  const source = data?.source ?? "none";
  // A failed fetch with nothing cached is its own state — never "caught
  // up", which is only true for a confirmed empty resolver result.
  const failed = isError && !data;
  const listSubtitle = nextUpListSubtitle(data);
  const heading =
    target?.title ??
    (target?.series_name && target?.number
      ? `${target.series_name} #${target.number}`
      : (target?.series_name ?? ""));
  const subheading =
    target?.title && target?.series_name && target?.number
      ? `${target.series_name} #${target.number}`
      : null;

  const [pane, setPane] = useState<"next" | "wrap">("next");
  // The card is always mounted; a reopen starts on Up Next again.
  // Render-phase adjust (React's "storing information from previous
  // renders" pattern) rather than an effect, so there is no extra
  // commit with the stale pane.
  const [lastOpen, setLastOpen] = useState(open);
  if (open !== lastOpen) {
    setLastOpen(open);
    if (!open) setPane("next");
  }

  // Focus the primary CTA on open so keyboard users can hit Enter to
  // continue without an extra Tab. requestAnimationFrame so the focus
  // happens after the slide-in animation starts. On dismiss, focus
  // returns to whatever held it before the card opened — without the
  // restore, an Esc-dismiss stranded keyboard users at document.body.
  const primaryButtonRef = useRef<HTMLButtonElement>(null);
  const primaryLinkRef = useRef<HTMLAnchorElement>(null);
  const backButtonRef = useRef<HTMLButtonElement>(null);
  const restoreFocusRef = useRef<HTMLElement | null>(null);
  useEffect(() => {
    if (!open) {
      const previous = restoreFocusRef.current;
      restoreFocusRef.current = null;
      if (previous && previous.isConnected) previous.focus();
      return;
    }
    restoreFocusRef.current =
      document.activeElement instanceof HTMLElement
        ? document.activeElement
        : null;
    const id = requestAnimationFrame(() => {
      if ((source !== "none" && target) || failed) {
        primaryButtonRef.current?.focus();
      } else {
        primaryLinkRef.current?.focus();
      }
    });
    return () => cancelAnimationFrame(id);
  }, [open, source, target, failed]);

  // Pane switch: focus the back arrow on the Wrap up pane (never the
  // textarea), and the primary CTA again when returning.
  const paneSwitchedRef = useRef(false);
  useEffect(() => {
    if (!open) return;
    if (!paneSwitchedRef.current) {
      paneSwitchedRef.current = true;
      return;
    }
    const id = requestAnimationFrame(() => {
      if (pane === "wrap") backButtonRef.current?.focus();
      else (primaryButtonRef.current ?? primaryLinkRef.current)?.focus();
    });
    return () => cancelAnimationFrame(id);
  }, [pane, open]);
  useEffect(() => {
    if (!open) paneSwitchedRef.current = false;
  }, [open]);

  // `1`–`5` rate the finished issue while the card is open. Capture
  // phase so the stroke never doubles as a reader shortcut; skipped
  // while typing in the closing-note textarea (`shouldSkipHotkey`).
  const onRate = wrapUp?.onRate;
  const rating = wrapUp?.rating ?? null;
  useEffect(() => {
    if (!open || !onRate) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.metaKey || e.ctrlKey || e.altKey) return;
      if (shouldSkipHotkey(e)) return;
      const next = ratingFromKey(e.key, rating);
      if (next === undefined) return;
      e.preventDefault();
      e.stopPropagation();
      onRate(next);
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [open, onRate, rating]);

  const slideFromRight = direction !== "rtl";
  const readLabel =
    source !== "none" && target ? `Read ${heading}` : "Read next";

  return (
    <div
      role="dialog"
      aria-modal="false"
      aria-label="End of issue"
      aria-hidden={!open}
      // The card is always mounted (CSS slide animation) — `inert`
      // keeps its links/buttons out of the tab order while closed.
      inert={open ? undefined : true}
      className={cn(
        // Floating card — vertically centered, small horizontal gap from
        // the screen edge so the page-turn arrows / chrome remain
        // visible. `max-h-[85vh]` keeps the card from outgrowing the
        // viewport on tiny windows.
        "fixed top-1/2 z-50 flex w-80 max-w-[85vw] -translate-y-1/2 flex-col",
        "bg-background text-foreground border-border rounded-lg border shadow-2xl",
        "max-h-[85vh] overflow-hidden",
        // CSS-only slide animation. `pointer-events-none` while closed
        // keeps the offscreen card from intercepting clicks on the
        // reader chrome below it.
        "transition-transform duration-300 ease-out",
        slideFromRight ? "right-4" : "left-4",
        open
          ? "translate-x-0"
          : slideFromRight
            ? "pointer-events-none translate-x-[calc(100%+1.5rem)]"
            : "pointer-events-none -translate-x-[calc(100%+1.5rem)]",
      )}
    >
      {pane === "wrap" && wrapUp ? (
        <>
          <div className="flex items-center gap-2 px-2.5 pt-3 pb-2">
            <button
              ref={backButtonRef}
              type="button"
              onClick={() => setPane("next")}
              aria-label="Back to Up Next"
              className="text-muted-foreground hover:text-foreground hover:bg-accent flex h-7 w-7 shrink-0 items-center justify-center rounded-md transition-colors"
            >
              <ChevronLeft className="size-4" />
            </button>
            <div className="min-w-0 flex-1">
              <p className="text-muted-foreground text-xs font-medium">
                Wrap up
              </p>
              <h2
                className="mt-0.5 truncate text-base leading-snug font-semibold"
                title={wrapUp.issueLabel}
              >
                {wrapUp.issueLabel}
              </h2>
            </div>
            <button
              type="button"
              onClick={onDismiss}
              aria-label="Close"
              className="text-muted-foreground hover:text-foreground hover:bg-accent flex h-7 w-7 shrink-0 items-center justify-center rounded-md transition-colors"
            >
              <X className="size-4" />
            </button>
          </div>
          <div className="min-h-0 overflow-y-auto">
            <WrapUpBody wrapUp={wrapUp} noteId="end-card-closing-note" />
          </div>
          <div className="border-border flex flex-col gap-2 border-t p-4">
            {source !== "none" && target ? (
              <Button onClick={onContinue} className="w-full">
                <span className="truncate">{readLabel}</span>
              </Button>
            ) : (
              <Button asChild variant="outline" className="w-full">
                <Link href={exitUrl} onClick={onDismiss}>
                  Exit reader
                </Link>
              </Button>
            )}
            <button
              type="button"
              onClick={() => setPane("next")}
              className="text-muted-foreground hover:text-foreground self-center text-xs transition-colors"
            >
              Back to Up Next
            </button>
          </div>
        </>
      ) : (
        <>
          <div className="flex items-start justify-between gap-3 p-4 pb-2">
            <div className="min-w-0 flex-1">
              <p className="text-muted-foreground text-xs font-medium">
                {failed
                  ? "End of issue"
                  : source === "none"
                    ? "End of the line"
                    : "Next up:"}
              </p>
              {source !== "none" && target ? (
                <h2 className="mt-0.5 text-base leading-snug font-semibold">
                  {heading}
                </h2>
              ) : failed ? (
                <h2 className="mt-0.5 text-base leading-snug font-semibold">
                  Couldn&apos;t load what&apos;s next
                </h2>
              ) : isLoading && !data ? (
                <div className="bg-muted mt-1 h-5 w-40 animate-pulse rounded" />
              ) : (
                <h2 className="mt-0.5 text-base leading-snug font-semibold">
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
              <X className="size-4" />
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
              {(subheading || listSubtitle) && (
                <div className="mt-2">
                  {subheading ? (
                    <p className="text-muted-foreground text-sm">
                      {subheading}
                    </p>
                  ) : null}
                  {listSubtitle ? (
                    <p className="text-muted-foreground mt-1 text-xs">
                      {listSubtitle}
                    </p>
                  ) : null}
                </div>
              )}
            </div>
          ) : failed ? (
            <div className="px-4 pb-3">
              <p className="text-muted-foreground text-sm" role="alert">
                The next-issue lookup failed. Your progress is saved.
              </p>
            </div>
          ) : isLoading && !data ? (
            <div className="px-4 pb-3">
              <div
                // Themed token, not hardcoded neutral-800: this card is
                // `bg-background` (theme-aware), so a dark block would clash
                // on light/amber themes. Matches the heading skeleton above.
                className="bg-muted aspect-[2/3] w-full animate-pulse rounded-md"
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
              <>
                <Button
                  ref={primaryButtonRef}
                  onClick={onContinue}
                  className="w-full"
                >
                  Read
                </Button>
                {/* Detail-page escape hatches: a matched outline pair for
                    the issue just finished (the chrome-exit destination)
                    and the resolver's pick, so the user can inspect either
                    issue page instead of committing to keep reading. The
                    label sits centered inside a hairline divider (the
                    auth-form "or continue with" treatment) so it reads as
                    a structural separator, not a stray caption. */}
                <div className="flex items-center gap-2">
                  <div className="bg-border h-px flex-1" aria-hidden="true" />
                  <span className="text-muted-foreground text-xs">
                    Go to issue page
                  </span>
                  <div className="bg-border h-px flex-1" aria-hidden="true" />
                </div>
                <div className="grid grid-cols-2 gap-2">
                  <Button asChild variant="outline">
                    <Link href={exitUrl} onClick={onDismiss}>
                      This issue
                    </Link>
                  </Button>
                  <Button asChild variant="outline">
                    <Link href={issueUrl(target)} onClick={onDismiss}>
                      Next issue
                    </Link>
                  </Button>
                </div>
              </>
            ) : failed ? (
              <>
                <Button
                  ref={primaryButtonRef}
                  onClick={onRetry}
                  className="w-full"
                  disabled={!onRetry}
                >
                  Retry
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

          {/* Bottom strip: rate the issue just finished, or flip to the
              Wrap up pane. Last so the Up Next flow above reads exactly
              as before; the darker band marks it as "this issue", not
              the next one. */}
          {wrapUp ? (
            <WrapUpStrip
              wrapUp={wrapUp}
              onOpen={() => setPane("wrap")}
              className="border-border border-t"
            />
          ) : null}
        </>
      )}
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
          className="truncate text-sm leading-snug font-medium"
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
