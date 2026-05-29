"use client";

import { useEffect } from "react";

import { StatusCard, StatusErrorIcon } from "@/components/StatusScreen";
import { Button } from "@/components/ui/button";

/**
 * Error boundary scoped to the (settings) route group:
 * /settings/{account,activity,views,api-tokens,...}. Renders inside
 * `AdminShell`, so it uses the bare `StatusCard`.
 */
export default function SettingsGroupError({
  error,
  reset,
}: {
  error: Error & { digest?: string };
  reset: () => void;
}) {
  useEffect(() => {
    console.error("(settings) error boundary:", error);
  }, [error]);

  return (
    <StatusCard
      icon={<StatusErrorIcon />}
      title="Couldn’t load this settings tab"
      description="Try again, or pick a different tab from the settings sidebar."
      digest={error.digest}
      actions={
        <>
          <Button onClick={reset}>Try again</Button>
          <Button asChild variant="outline">
            <a href="/settings">Settings home</a>
          </Button>
        </>
      }
    />
  );
}
