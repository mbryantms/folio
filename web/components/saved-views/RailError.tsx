"use client";

import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

/**
 * Shared failure state for a rail / list body whose fetch failed with
 * nothing cached (post-audit T2.4). Distinct from the empty state: an
 * empty rail hides itself, a failed one says so and offers Retry.
 * Without `onRetry` the button reloads the page (server-rendered
 * callers that have no query to refetch).
 */
export function RailError({
  message = "Couldn't load this rail.",
  retryLabel = "Retry",
  onRetry,
  className,
}: {
  message?: string;
  retryLabel?: string;
  onRetry?: () => void;
  className?: string;
}) {
  const retry =
    onRetry ??
    (() => {
      if (typeof window !== "undefined") window.location.reload();
    });
  return (
    <div
      role="alert"
      className={cn(
        "text-muted-foreground flex flex-wrap items-center gap-3 text-sm",
        className,
      )}
    >
      <span>{message}</span>
      <Button size="sm" variant="outline" onClick={retry}>
        {retryLabel}
      </Button>
    </div>
  );
}
