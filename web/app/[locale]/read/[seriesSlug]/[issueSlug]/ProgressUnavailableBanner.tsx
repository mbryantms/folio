"use client";

/**
 * Shown when the server-side fetch of the user's saved position failed
 * (post-audit T2.2). The reader used to start silently at page 0 in that
 * case, which looks exactly like "no progress yet"; this says what
 * happened and offers a reload so the saved page is used. Same fixed
 * top-of-viewport treatment as the peek banner.
 */

import { useState } from "react";

export function ProgressUnavailableBanner({
  onRetry,
}: {
  /** Defaults to a full reload — the position is read server-side. */
  onRetry?: () => void;
}) {
  const [dismissed, setDismissed] = useState(false);
  if (dismissed) return null;
  const retry =
    onRetry ??
    (() => {
      if (typeof window !== "undefined") window.location.reload();
    });
  return (
    <div
      className="pointer-events-none fixed inset-x-0 top-0 z-30 flex justify-center px-3 pt-(--safe-top)"
      role="alert"
    >
      <div className="border-border bg-background/95 text-foreground pointer-events-auto mt-3 flex w-full max-w-md items-center gap-3 rounded-lg border px-3 py-2 shadow-lg backdrop-blur">
        <p className="flex-1 text-xs leading-tight">
          <span className="font-medium">
            Couldn&apos;t load your saved position.
          </span>{" "}
          <span className="text-muted-foreground">
            Starting from the first page; reload to resume where you left off.
          </span>
        </p>
        <button
          type="button"
          onClick={retry}
          className="bg-primary text-primary-foreground hover:bg-primary/90 shrink-0 rounded-md px-3 py-1.5 text-xs font-medium"
        >
          Retry
        </button>
        <button
          type="button"
          onClick={() => setDismissed(true)}
          aria-label="Dismiss"
          className="text-muted-foreground hover:text-foreground shrink-0 rounded-md px-2 py-1.5 text-xs"
        >
          ✕
        </button>
      </div>
    </div>
  );
}
