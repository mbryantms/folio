"use client";

import * as React from "react";
import { useRouter } from "next/navigation";
import { Play } from "lucide-react";

import { jsonFetch } from "@/lib/api/queries";
import type { SeriesResumeView } from "@/lib/api/types";
import { cn } from "@/lib/utils";

/**
 * Hover/focus-revealed "jump to reader" affordance that overlays a cover.
 *
 * The card it lives on is typically already wrapped in a `<Link>` to the
 * issue/series detail page. This overlay short-circuits that navigation
 * for users who just want to read: clicking the play indicator stops
 * propagation, prevents the parent link from firing, and routes to the
 * reader instead.
 *
 * Markup choice: rendered as a `<span role="button">` rather than a
 * `<button>` or nested `<a>`, because either of those forms inside the
 * parent `<Link>`'s anchor would be invalid HTML5 (and Next.js dev mode
 * warns about it). The span carries `tabIndex={0}` and Enter/Space key
 * handlers so it's still keyboard-reachable; programmatic navigation
 * uses `next/navigation`'s `useRouter().push`.
 *
 * Visibility: hidden by default; revealed when the **nearest ancestor
 * with the `group` class** is hovered, or when either the overlay itself
 * or anything inside the group has keyboard focus. The parent card MUST
 * set `class="group"` on the outermost element wrapping the cover. The
 * existing `SeriesCard` / `IssueCard` / `CblIssueCard` already do.
 *
 * Positioning: `absolute right-2 bottom-2` of the nearest positioned
 * ancestor — typically the cover wrapper (a `relative` div hosting the
 * `<Cover>` image). When dropped into a card, place it as a sibling of
 * `<Cover>` inside the cover's relative wrapper.
 */
export function QuickReadOverlay({
  readerHref,
  label,
  className,
}: {
  readerHref: string;
  /** Required — describes the action contextually (e.g. "Continue reading
   *  Saga #1 from page 7"). */
  label: string;
  className?: string;
}) {
  const router = useRouter();
  const activate = () => router.push(readerHref);
  return (
    <span
      role="button"
      tabIndex={0}
      aria-label={label}
      title={label}
      onClick={(e) => {
        // The cover sits inside a parent <Link>. Stopping propagation +
        // preventing default keeps the parent navigation from firing
        // alongside our reader push.
        e.preventDefault();
        e.stopPropagation();
        activate();
      }}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          e.stopPropagation();
          activate();
        }
      }}
      className={cn(
        "absolute right-2 bottom-2 z-10",
        // Fixed 32px footprint — never grows with cover size so it stays
        // out of the way on dense rails and consistent across cards.
        "bg-primary/90 text-primary-foreground inline-flex h-8 w-8 cursor-pointer items-center justify-center rounded-full ring-2 shadow-md ring-white/20 backdrop-blur",
        "scale-90 opacity-0 transition-all duration-150 ease-out",
        "group-hover:scale-100 group-hover:opacity-100",
        "group-focus-within:scale-100 group-focus-within:opacity-100",
        "focus-visible:scale-100 focus-visible:opacity-100 focus-visible:ring-offset-2 focus-visible:outline-none",
        className,
      )}
    >
      <Play className="h-3.5 w-3.5 fill-current pl-0.5" aria-hidden="true" />
    </span>
  );
}

/**
 * Series-scoped variant of `<QuickReadOverlay>`. Doesn't know the resume
 * target up-front; resolves it on click via `GET /series/{slug}/resume`
 * (which mirrors the client-side `pickNextIssue` algorithm) and then
 * navigates to the reader. Avoids the cost of pre-fetching progress for
 * every series tile rendered in the library/grid.
 *
 * Visually identical to `<QuickReadOverlay>` — same fixed footprint,
 * same hover-reveal mechanics. The intermediate "resolving…" state
 * after click sets `aria-busy`; clients without a resume target receive
 * a no-op (e.g. an empty series with no readable issues).
 */
export function SeriesPlayOverlay({
  seriesSlug,
  seriesName,
  className,
}: {
  seriesSlug: string;
  /** Used to build a contextual ARIA label like "Read Saga". */
  seriesName: string;
  className?: string;
}) {
  const router = useRouter();
  const [busy, setBusy] = React.useState(false);
  const label = `Read ${seriesName}`;

  const activate = async () => {
    if (busy) return;
    setBusy(true);
    try {
      const resume = await jsonFetch<SeriesResumeView>(
        `/series/${encodeURIComponent(seriesSlug)}/resume`,
      );
      if (!resume.issue_slug) return;
      router.push(
        `/read/${encodeURIComponent(resume.series_slug)}/${encodeURIComponent(resume.issue_slug)}`,
      );
    } catch {
      // Quiet failure — the play button is a shortcut, not the only
      // path to the reader. The card's main click still works.
    } finally {
      setBusy(false);
    }
  };

  return (
    <span
      role="button"
      tabIndex={0}
      aria-label={label}
      aria-busy={busy}
      title={label}
      onClick={(e) => {
        e.preventDefault();
        e.stopPropagation();
        void activate();
      }}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          e.stopPropagation();
          void activate();
        }
      }}
      className={cn(
        "absolute right-2 bottom-2 z-10",
        "bg-primary/90 text-primary-foreground inline-flex h-8 w-8 cursor-pointer items-center justify-center rounded-full ring-2 shadow-md ring-white/20 backdrop-blur",
        "scale-90 opacity-0 transition-all duration-150 ease-out",
        "group-hover:scale-100 group-hover:opacity-100",
        "group-focus-within:scale-100 group-focus-within:opacity-100",
        "focus-visible:scale-100 focus-visible:opacity-100 focus-visible:ring-offset-2 focus-visible:outline-none",
        // While the resume fetch is in flight, soften the icon so the
        // user knows the click registered.
        busy && "opacity-100",
        className,
      )}
    >
      <Play className="h-3.5 w-3.5 fill-current pl-0.5" aria-hidden="true" />
    </span>
  );
}
