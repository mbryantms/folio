"use client";

import { useEffect } from "react";
import { AlertCircle } from "lucide-react";

import { Button } from "@/components/ui/button";

/**
 * Error boundary scoped to the (library) route group: home, series,
 * collections, views, bookmarks, log, creators, pages, search.
 * Falls back to the locale-level error.tsx if it throws.
 * Audit-remediation M7.1 (2026-05-24).
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
    <div className="mx-auto max-w-md py-16 text-center">
      <div className="mx-auto flex size-10 items-center justify-center rounded-full border border-destructive/40 bg-destructive/10 text-destructive">
        <AlertCircle className="size-5" aria-hidden />
      </div>
      <h2 className="mt-4 text-lg font-semibold">Couldn’t load this page.</h2>
      <p className="mt-2 text-sm text-muted-foreground">
        Something went sideways while fetching your library data.
      </p>
      {error.digest ? (
        <p className="mt-3 font-mono text-[11px] text-muted-foreground/70">
          ref: {error.digest}
        </p>
      ) : null}
      <div className="mt-6 flex items-center justify-center gap-2">
        <Button onClick={reset} size="sm">
          Try again
        </Button>
        <Button asChild size="sm" variant="outline">
          <a href="/">Home</a>
        </Button>
      </div>
    </div>
  );
}
