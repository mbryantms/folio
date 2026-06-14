"use client";

import * as React from "react";

const MESSAGE = "You have unsaved changes. Leave this page?";

/**
 * Warn before navigating away from a form with unsaved edits (audit D6 — the
 * long admin forms previously discarded silently one click from a sibling tab).
 *
 * Two layers, because the App Router exposes no navigation-block API:
 *  1. `beforeunload` covers reload / tab-close / external navigation.
 *  2. A capture-phase document click handler intercepts same-origin in-app
 *     `<a>`/`<Link>` clicks and `confirm()`s before letting them through.
 *     (Doesn't cover purely programmatic `router.push`, which admin nav rarely
 *     uses — links/tabs are the data-loss path.)
 *
 * Both layers are inert when `isDirty` is false, so passing a clean form is a
 * no-op.
 */
export function useUnsavedChangesGuard(isDirty: boolean) {
  React.useEffect(() => {
    if (!isDirty) return;

    const onBeforeUnload = (e: BeforeUnloadEvent) => {
      e.preventDefault();
      // Legacy Chrome requires a returnValue assignment to trigger the prompt.
      e.returnValue = "";
    };

    const onClickCapture = (e: MouseEvent) => {
      // Let modified clicks (new tab / download / middle-click) and already
      // handled events pass.
      if (
        e.defaultPrevented ||
        e.button !== 0 ||
        e.metaKey ||
        e.ctrlKey ||
        e.shiftKey ||
        e.altKey
      ) {
        return;
      }
      const anchor = (e.target as HTMLElement | null)?.closest?.("a[href]") as
        | HTMLAnchorElement
        | null;
      if (!anchor || anchor.target === "_blank" || anchor.hasAttribute("download")) {
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
      if (!window.confirm(MESSAGE)) {
        e.preventDefault();
        e.stopPropagation();
      }
    };

    window.addEventListener("beforeunload", onBeforeUnload);
    document.addEventListener("click", onClickCapture, true);
    return () => {
      window.removeEventListener("beforeunload", onBeforeUnload);
      document.removeEventListener("click", onClickCapture, true);
    };
  }, [isDirty]);
}
