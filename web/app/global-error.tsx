"use client";

// global-error replaces the root layout when an error is thrown in it
// (or in `[locale]/layout`), so it must render its own <html>/<body> and
// import the global stylesheet itself — the root layout never ran.
import "@/styles/globals.css";

import { useEffect } from "react";

import { StatusScreen, StatusErrorIcon } from "@/components/StatusScreen";
import { Button } from "@/components/ui/button";

/**
 * Last-resort boundary for crashes in the root/locale layout. The theme
 * cookie can't be read here (no ThemeProvider), so we pin `data-theme`
 * to the app's dark default; tokens still resolve via globals.css.
 */
export default function GlobalError({
  error,
  reset,
}: {
  error: Error & { digest?: string };
  reset: () => void;
}) {
  useEffect(() => {
    console.error("GlobalError (root) boundary caught:", error);
  }, [error]);

  return (
    <html lang="en" data-theme="dark" suppressHydrationWarning>
      <body className="bg-background text-foreground antialiased">
        <StatusScreen
          icon={<StatusErrorIcon />}
          title="Something went wrong"
          description="The app failed to load. Reloading usually clears it."
          digest={error.digest}
          actions={
            <>
              <Button onClick={reset}>Try again</Button>
              <Button asChild variant="outline">
                <a href="/">Reload</a>
              </Button>
            </>
          }
        />
      </body>
    </html>
  );
}
