"use client";

import * as React from "react";

import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";

/** Set while a confirmed click is replayed, so no guard on the page (one
 *  form may mount several) stops it again. */
let replaying = false;

/**
 * Warn before navigating away from a form with unsaved edits (audit D6 — the
 * long admin forms previously discarded silently one click from a sibling tab).
 *
 * Two layers, because the App Router exposes no navigation-block API:
 *  1. `beforeunload` covers reload / tab-close / external navigation (the
 *     browser's own prompt; pages cannot style it).
 *  2. A capture-phase document click handler intercepts same-origin in-app
 *     `<a>`/`<Link>` clicks and asks first in an `AlertDialog`. Leaving
 *     replays the click on the same anchor, so a `<Link>` still navigates
 *     client-side. (Doesn't cover purely programmatic `router.push`, which
 *     admin nav rarely uses — links/tabs are the data-loss path.)
 *
 * Both layers are inert when `isDirty` is false, so passing a clean form is a
 * no-op. Returns the confirmation dialog; render it anywhere in the form.
 */
export function useUnsavedChangesGuard(isDirty: boolean): React.ReactNode {
  const [pending, setPending] = React.useState<HTMLAnchorElement | null>(null);

  React.useEffect(() => {
    if (!isDirty) return;

    const onBeforeUnload = (e: BeforeUnloadEvent) => {
      e.preventDefault();
      // Legacy Chrome requires a returnValue assignment to trigger the prompt.
      e.returnValue = "";
    };

    const onClickCapture = (e: MouseEvent) => {
      // Let modified clicks (new tab / download / middle-click), already
      // handled events and the confirmed replay pass.
      if (
        replaying ||
        e.defaultPrevented ||
        e.button !== 0 ||
        e.metaKey ||
        e.ctrlKey ||
        e.shiftKey ||
        e.altKey
      ) {
        return;
      }
      const anchor = (e.target as HTMLElement | null)?.closest?.(
        "a[href]",
      ) as HTMLAnchorElement | null;
      if (
        !anchor ||
        anchor.target === "_blank" ||
        anchor.hasAttribute("download")
      ) {
        return;
      }
      const href = anchor.getAttribute("href");
      if (!href || href.startsWith("#")) return;
      const dest = new URL(anchor.href, window.location.href);
      // Only guard in-app navigation to a different page.
      if (dest.origin !== window.location.origin) return;
      if (
        dest.pathname === window.location.pathname &&
        dest.search === window.location.search
      ) {
        return;
      }
      e.preventDefault();
      // Immediate: a second dirty form on the page must not ask again.
      e.stopImmediatePropagation();
      setPending(anchor);
    };

    window.addEventListener("beforeunload", onBeforeUnload);
    document.addEventListener("click", onClickCapture, true);
    return () => {
      window.removeEventListener("beforeunload", onBeforeUnload);
      document.removeEventListener("click", onClickCapture, true);
    };
  }, [isDirty]);

  const leave = () => {
    const anchor = pending;
    setPending(null);
    if (!anchor?.isConnected) return;
    replaying = true;
    try {
      anchor.click();
    } finally {
      replaying = false;
    }
  };

  return (
    <AlertDialog
      open={pending !== null}
      onOpenChange={(open) => {
        if (!open) setPending(null);
      }}
    >
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>Leave this page?</AlertDialogTitle>
          <AlertDialogDescription>
            You have unsaved changes. Leaving discards them.
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>Stay</AlertDialogCancel>
          <AlertDialogAction variant="destructive" onClick={leave}>
            Leave page
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
