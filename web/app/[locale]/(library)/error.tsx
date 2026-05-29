"use client";

import { useEffect } from "react";

import { StatusCard, StatusErrorIcon } from "@/components/StatusScreen";
import { Button } from "@/components/ui/button";

/**
 * Error boundary scoped to the (library) route group: home, series,
 * collections, views, bookmarks, log, creators, pages, search. Renders
 * inside `MainShell`, so it uses the bare `StatusCard` (no brand frame).
 * Falls back to the locale-level error.tsx if it throws.
 */
export default function LibraryGroupError({
  error,
  reset,
}: {
  error: Error & { digest?: string };
  reset: () => void;
}) {
  useEffect(() => {
    console.error("(library) error boundary:", error);
  }, [error]);

  return (
    <StatusCard
      icon={<StatusErrorIcon />}
      title="Couldn’t load this page"
      description="Something went sideways while fetching your library data."
      digest={error.digest}
      actions={
        <>
          <Button onClick={reset}>Try again</Button>
          <Button asChild variant="outline">
            <a href="/">Home</a>
          </Button>
        </>
      }
    />
  );
}
