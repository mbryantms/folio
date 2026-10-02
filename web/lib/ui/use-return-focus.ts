"use client";

import * as React from "react";

/**
 * Focus return for **controlled** Radix dialogs (no `DialogTrigger` /
 * `AlertDialogTrigger`).
 *
 * Radix's dialog content always cancels its focus scope's own return and
 * focuses the dialog's trigger instead — with no trigger, focus lands on
 * `<body>` when the dialog closes, so a keyboard user is thrown back to
 * the top of the page. Call `capture()` in the handler that opens the
 * dialog (it records the focused opener) and pass `onCloseAutoFocus` to
 * the `DialogContent` / `AlertDialogContent`; when the opener is still in
 * the document it gets focus back.
 */
export function useReturnFocus() {
  const opener = React.useRef<HTMLElement | null>(null);
  const capture = React.useCallback(() => {
    const el = document.activeElement;
    opener.current = el instanceof HTMLElement ? el : null;
  }, []);
  const onCloseAutoFocus = React.useCallback((e: Event) => {
    const el = opener.current;
    opener.current = null;
    if (el?.isConnected) {
      e.preventDefault();
      el.focus();
    }
  }, []);
  return { capture, onCloseAutoFocus };
}
