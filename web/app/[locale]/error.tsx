"use client";

import { useEffect } from "react";

import { StatusScreen, StatusErrorIcon } from "@/components/StatusScreen";
import { Button } from "@/components/ui/button";

/**
 * App Router error boundary for everything under `[locale]/`. Next.js
 * requires this to be a client component (it catches throws from the
 * rendering subtree, which only client code can do). Renders outside the
 * app shell, so it uses the shared `StatusScreen` brand frame.
 */
export default function LocaleError({
  error,
  reset,
}: {
  error: Error & { digest?: string };
  reset: () => void;
}) {
  useEffect(() => {
    // Surface to the browser console so devtools-open users see the
    // stack; the digest is what Next.js logs server-side, so quoting
    // it makes correlation easy when an operator pulls the row from
    // `/admin/logs`.
    console.error("LocaleError boundary caught:", error);
  }, [error]);

  return (
    <StatusScreen
      icon={<StatusErrorIcon />}
      title="Something went wrong"
      description="The page hit an unexpected error. You can try again, or head back to the library and pick up where you left off."
      digest={error.digest}
      actions={
        <>
          <Button onClick={reset}>Try again</Button>
          <Button asChild variant="outline">
            {/* Hard nav escapes the broken React subtree. */}
            <a href="/">Back to library</a>
          </Button>
        </>
      }
    />
  );
}
