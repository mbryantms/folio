"use client";

import { useSyncExternalStore } from "react";
import { toast } from "sonner";

import { useCopyToClipboard } from "@/components/ui/copy-button";

/**
 * Device-adaptive "share or copy link" affordance (data-liberation 3.3).
 *
 * On a device that supports the Web Share API → the native
 * share sheet (label "Share"); everywhere else → clipboard copy (label
 * "Copy link"). Callers pass an in-app path (e.g. `seriesUrl(series)`),
 * which is resolved to an absolute URL against the current origin so the
 * shared/copied link works when pasted elsewhere.
 */
export function useShareLink() {
  const { copy } = useCopyToClipboard();
  const canShare = useSyncExternalStore(
    () => () => {},
    () =>
      typeof navigator !== "undefined" && typeof navigator.share === "function",
    () => false,
  );
  const label = canShare ? "Share" : "Copy link";

  async function copyLink(path: string): Promise<void> {
    const url = path.startsWith("/")
      ? `${window.location.origin}${path}`
      : path;
    const ok = await copy(url);
    toast[ok ? "success" : "error"](ok ? "Link copied" : "Couldn't copy link");
  }

  async function shareOrCopy(path: string, title?: string): Promise<void> {
    const url =
      typeof window !== "undefined" && path.startsWith("/")
        ? `${window.location.origin}${path}`
        : path;
    if (canShare) {
      try {
        await navigator.share({ url, title });
        return;
      } catch (error) {
        if (error instanceof DOMException && error.name === "AbortError")
          return;
        // Permission / platform failures fall back to copying the link.
      }
    }
    await copyLink(url);
  }

  return { label, shareOrCopy, copyLink, canShare };
}
