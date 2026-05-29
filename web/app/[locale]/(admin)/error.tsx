"use client";

import { useEffect } from "react";

import { StatusCard, StatusErrorIcon } from "@/components/StatusScreen";
import { Button } from "@/components/ui/button";

/**
 * Error boundary scoped to the (admin) route group: /admin/users,
 * /admin/libraries, /admin/audit, /admin/settings, etc. Admin pages are
 * richer (charts, tables, mutation forms) so a localized boundary lets
 * the user retry one tab without leaving /admin entirely. Renders inside
 * `AdminShell`, so it uses the bare `StatusCard`.
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
    <StatusCard
      icon={<StatusErrorIcon />}
      title="Admin page failed to load"
      description="The request hit an unexpected error. Other admin sections may still work."
      digest={error.digest}
      actions={
        <>
          <Button onClick={reset}>Try again</Button>
          <Button asChild variant="outline">
            <a href="/admin">Admin home</a>
          </Button>
        </>
      }
    />
  );
}
