"use client";

import * as React from "react";

/**
 * Snapshot a container's content-box width and keep it in sync via
 * `ResizeObserver`. Returns `0` until the first layout pass — callers
 * should short-circuit rendering (or fall back to a skeleton) during
 * that interval to avoid a flash or a divide-by-zero in layout math.
 *
 * Shared by the bookmarks row-packed grid (`MarkersList`) and the
 * window-virtualized library grid (`LibraryGridView`), both of which
 * need the live container width to compute their layout.
 */
export function useContainerWidth<E extends HTMLElement>(): [
  React.RefObject<E | null>,
  number,
] {
  const ref = React.useRef<E | null>(null);
  const [width, setWidth] = React.useState(0);
  React.useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const ro = new ResizeObserver((entries) => {
      const w = entries[0]?.contentRect.width;
      if (typeof w === "number") setWidth(w);
    });
    ro.observe(el);
    setWidth(el.clientWidth);
    return () => ro.disconnect();
  }, []);
  return [ref, width];
}
