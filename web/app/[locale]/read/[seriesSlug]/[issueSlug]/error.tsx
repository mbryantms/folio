"use client";

import { useEffect, useMemo } from "react";
import { usePathname } from "next/navigation";

import { Button } from "@/components/ui/button";

/**
 * Error boundary scoped to the reader route. The reader lives outside
 * the `(library)` group, so without this file a mid-read crash fell
 * through to the generic `[locale]/error.tsx` — a light-theme library
 * shell with a "Back to library" action that threw away the reading
 * context entirely. This boundary keeps the reader's dark surface
 * (`--reader-bg`, matching `loading.tsx`) and offers a path back to
 * the issue the user was just reading.
 *
 * The issue-detail URL is derived from the pathname
 * (`/read/{seriesSlug}/{issueSlug}` → `/series/{s}/issues/{i}`) —
 * error boundaries don't receive route params.
 */
export default function ReaderError({
  error,
  reset,
}: {
  error: Error & { digest?: string };
  reset: () => void;
}) {
  useEffect(() => {
    console.error("reader error boundary:", error);
  }, [error]);

  const pathname = usePathname();
  const issueUrl = useMemo(() => {
    const m = pathname?.match(/^\/read\/([^/]+)\/([^/]+)/);
    return m ? `/series/${m[1]}/issues/${m[2]}` : "/";
  }, [pathname]);

  return (
    <div className="bg-reader-bg grid min-h-screen place-items-center px-6 text-neutral-200">
      <div className="flex max-w-sm flex-col items-center gap-4 text-center">
        <h1 className="text-lg font-semibold">The reader hit a snag</h1>
        <p className="text-sm text-neutral-400">
          Your reading position is saved. You can retry right here or head
          back to the issue page.
        </p>
        {error.digest ? (
          <p className="font-mono text-xs text-neutral-600">
            digest: {error.digest}
          </p>
        ) : null}
        <div className="flex items-center gap-2">
          <Button onClick={reset}>Try again</Button>
          <Button asChild variant="outline">
            <a href={issueUrl}>Back to issue</a>
          </Button>
        </div>
      </div>
    </div>
  );
}
