"use client";

import { useEffect } from "react";
import { AlertCircle } from "lucide-react";

import { Button } from "@/components/ui/button";

/**
 * Error boundary scoped to the (admin) route group: /admin/users,
 * /admin/libraries, /admin/audit, /admin/settings, etc. Admin pages
 * are richer (charts, tables, mutation forms) so a localized boundary
 * lets the user retry one tab without leaving /admin entirely.
 * Audit-remediation M7.1 (2026-05-24).
 */
export default function AdminGroupError({
  error,
  reset,
}: {
  error: Error & { digest?: string };
  reset: () => void;
}) {
  useEffect(() => {
    console.error("(admin) error boundary:", error);
  }, [error]);

  return (
    <div className="mx-auto max-w-md py-16 text-center">
      <div className="mx-auto flex size-10 items-center justify-center rounded-full border border-destructive/40 bg-destructive/10 text-destructive">
        <AlertCircle className="size-5" aria-hidden />
      </div>
      <h2 className="mt-4 text-lg font-semibold">Admin page failed to load.</h2>
      <p className="mt-2 text-sm text-muted-foreground">
        The request hit an unexpected error. Other admin sections may still
        work.
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
          <a href="/admin">Admin home</a>
        </Button>
      </div>
    </div>
  );
}
