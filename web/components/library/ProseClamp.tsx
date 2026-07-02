"use client";

import * as React from "react";

/** Character count above which mobile clamping + "Read more" kicks
 *  in. Short summaries (a couple sentences) wouldn't visibly clamp
 *  under `line-clamp-3` anyway, so we skip the toggle for them
 *  entirely — the button on its own line is more noise than help
 *  when the text already fits. */
const MIN_CLAMP_LENGTH = 200;

/** Prose paragraph that clamps to 3 lines on mobile with a toggle
 *  back to full-height. sm+ always renders the full paragraph (no
 *  clamp, no toggle button) since vertical space isn't scarce there.
 *
 *  Client-only because we need state for the toggle. Plain prose is
 *  the only non-server-friendly branch of `<Description>`; the
 *  structured-content branch (variant-cover tables, etc.) stays
 *  server-rendered. */
export function ProseClamp({ text }: { text: string }) {
  const [expanded, setExpanded] = React.useState(false);
  const needsClamp = text.length >= MIN_CLAMP_LENGTH;
  if (!needsClamp) {
    return (
      <p className="text-foreground/90 max-w-prose text-sm leading-6">{text}</p>
    );
  }
  return (
    <div className="max-w-prose">
      <p
        className={
          "text-foreground/90 text-sm leading-6 " +
          (expanded ? "" : "line-clamp-3 sm:line-clamp-none")
        }
      >
        {text}
      </p>
      {/* Toggle is mobile-only — sm+ never clamps so the button would
          be a permanently-disabled affordance. Tiny height so it
          doesn't stretch the description's vertical rhythm. */}
      <button
        type="button"
        onClick={() => setExpanded((v) => !v)}
        className="text-primary mt-1 text-xs font-medium sm:hidden"
        aria-expanded={expanded}
      >
        {expanded ? "Show less" : "Read more"}
      </button>
    </div>
  );
}
