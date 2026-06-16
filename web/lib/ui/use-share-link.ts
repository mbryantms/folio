"use client";

import { toast } from "sonner";

import { useCopyToClipboard } from "@/components/ui/copy-button";
import { useCoarsePointer } from "@/lib/ui/use-coarse-pointer";

/**
 * Device-adaptive "share or copy link" affordance (data-liberation 3.3).
 *
 * On a coarse-pointer device that supports the Web Share API → the native
 * share sheet (label "Share"); everywhere else → clipboard copy (label
 * "Copy link"). Callers pass an in-app path (e.g. `seriesUrl(series)`),
 * which is resolved to an absolute URL against the current origin so the
 * shared/copied link works when pasted elsewhere.
 */
export function useShareLink() {
  const coarse = useCoarsePointer();
  const { copy } = useCopyToClipboard();
  const canShare =
    coarse &&
    typeof navigator !== "undefined" &&
    typeof navigator.share === "function";
  const label = canShare ? "Share" : "Copy link";

  async function shareOrCopy(path: string, title?: string): Promise<void> {
    const url =
      typeof window !== "undefined" && path.startsWith("/")
        ? `${window.location.origin}${path}`
        : path;
    if (canShare) {
      try {
        await navigator.share({ url, title });
      } catch {
        // AbortError when the user dismisses the share sheet — not a failure.
      }
      return;
    }
    const ok = await copy(url);
    toast[ok ? "success" : "error"](ok ? "Link copied" : "Couldn't copy link");
  }

  return { label, shareOrCopy };
}
