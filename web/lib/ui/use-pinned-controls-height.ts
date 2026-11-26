"use client";

import * as React from "react";

/** The rows a list page pins under the topbar: the desktop toolbar while
 *  it is sticky, the phone controls bar, or the selection toolbar that
 *  replaces either while selecting. */
const PINNED_SELECTOR =
  "[data-toolbar-row][data-sticky], [data-phone-controls], .selection-toolbar-wrap";

/** The CSS variable {@link usePinnedControlsHeight} publishes. */
export const PINNED_CONTROLS_VAR = "--pinned-controls-h";

/** Height of the visible pinned control row inside `root`, or 0. Pure DOM
 *  read, exported for tests. */
export function pinnedControlsHeight(root: ParentNode): number {
  for (const el of root.querySelectorAll<HTMLElement>(PINNED_SELECTOR)) {
    // `display: none` (the other breakpoint's bar) has no offset parent
    // and no height.
    if (el.offsetHeight === 0) continue;
    if (getComputedStyle(el).position !== "sticky") continue;
    return el.offsetHeight;
  }
  return 0;
}

/**
 * Publish the height of the page's pinned control row as
 * `--pinned-controls-h` on the element given the returned ref callback, so content that
 * pins below it (chapter headers, a side rail) and scroll margins can
 * sit under `--topbar-h` + that instead of a guessed constant.
 *
 * Written to the element's style, not React state: it changes when the
 * toolbar wraps, sticks or is swapped for the selection toolbar, and
 * nothing needs to re-render for it. `0px` when no row is pinned.
 */
export function usePinnedControlsHeight<T extends HTMLElement>() {
  // A callback ref with cleanup (React 19): the page root often mounts
  // after an early loading return, which a mount-time effect would miss.
  return React.useCallback((root: T | null) => {
    if (!root) return;
    let frame = 0;
    const update = () => {
      frame = 0;
      root.style.setProperty(
        PINNED_CONTROLS_VAR,
        `${pinnedControlsHeight(root)}px`,
      );
    };
    const schedule = () => {
      if (!frame) frame = requestAnimationFrame(update);
    };
    update();
    // Rows mount, unmount, wrap, open and flip `data-sticky`; one
    // observer on the page root covers all of it.
    const mo = new MutationObserver(schedule);
    mo.observe(root, {
      subtree: true,
      childList: true,
      attributes: true,
      attributeFilter: ["data-sticky", "data-state"],
    });
    const ro =
      typeof ResizeObserver === "undefined"
        ? null
        : new ResizeObserver(schedule);
    ro?.observe(root);
    window.addEventListener("resize", schedule);
    return () => {
      if (frame) cancelAnimationFrame(frame);
      mo.disconnect();
      ro?.disconnect();
      window.removeEventListener("resize", schedule);
    };
  }, []);
}
