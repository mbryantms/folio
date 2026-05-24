"use client";

import { useEffect } from "react";
import { AlertCircle } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Chrome } from "@/components/Chrome";

/**
 * App Router error boundary for everything under `[locale]/`. Next.js
 * requires this to be a client component (it catches throws from the
 * rendering subtree, which only client code can do). Audit-remediation
 * M7.1 (2026-05-24).
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
    <Chrome breadcrumbs={[{ label: "Something went wrong" }]}>
      <div className="mx-auto max-w-md py-16 text-center">
        <div className="mx-auto flex size-12 items-center justify-center rounded-full border border-destructive/40 bg-destructive/10 text-destructive">
          <AlertCircle className="size-6" aria-hidden />
        </div>
        <h1 className="mt-6 text-2xl font-semibold tracking-tight">
          Something went wrong.
        </h1>
        <p className="mt-3 text-sm text-muted-foreground">
          The page hit an unexpected error. You can try again, or head back
          to the library and pick up where you left off.
        </p>
        {error.digest ? (
          <p className="mt-4 font-mono text-[11px] text-muted-foreground/70">
            ref: {error.digest}
          </p>
        ) : null}
        <div className="mt-8 flex items-center justify-center gap-3">
          <Button onClick={reset} variant="default">
            Try again
          </Button>
          <Button asChild variant="outline">
            <a href="/">Back to library</a>
          </Button>
        </div>
      </div>
    </Chrome>
  );
}
