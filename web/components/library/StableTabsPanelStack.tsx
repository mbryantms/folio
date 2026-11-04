"use client";

import * as React from "react";

import { TabsContent } from "@/components/folio/tabs";
import { cn } from "@/lib/utils";

const STACKED_PANEL_CLASS = "col-start-1 row-start-1 pt-6";
const INACTIVE_STABLE_PANEL_CLASS =
  "data-[state=inactive]:pointer-events-none data-[state=inactive]:invisible";

export function StableTabsPanelStack({
  children,
  className,
}: {
  children: React.ReactNode;
  className?: string;
}) {
  const ref = React.useRef<HTMLDivElement>(null);
  useEasedPanelHeight(ref);
  // `minmax(0, 1fr)`: the one column is the container's width, never the
  // content's — a horizontal rail inside a panel (Related tab, WP-7.7)
  // would otherwise widen the implicit `auto` column and the page.
  // `content-start items-start`: panels keep their own height while the
  // stack's height is being eased (a stretched row would report the
  // container's height back as the panel's).
  return (
    <div
      ref={ref}
      className={cn(
        "grid grid-cols-[minmax(0,1fr)] content-start items-start",
        className,
      )}
    >
      {children}
    </div>
  );
}

/** How long the stack takes to grow or shrink to the new tab's height. */
export const PANEL_HEIGHT_MS = 420;
/** How long a switch waits for a lazy panel's content before letting go. */
const LATE_CONTENT_MS = 250;

/**
 * Ease the stack's height when the tab changes (folio-flair): a short tab
 * after a tall one (Credits after Metadata) no longer snaps the page up,
 * and a tall one grows into place. Only tab switches animate (a panel's
 * `data-state` flips or a lazy panel mounts/unmounts); content growing
 * inside a panel later is left alone. While easing, the stack clips
 * vertically and retargets if the new panel's content loads mid-way.
 * Reduced motion: no easing.
 */
function useEasedPanelHeight(ref: React.RefObject<HTMLDivElement | null>) {
  React.useEffect(() => {
    const el = ref.current;
    if (!el || typeof ResizeObserver === "undefined") return;
    const natural = () =>
      Math.max(
        0,
        ...[...el.children].map((c) => (c as HTMLElement).offsetHeight),
      );
    let settled = el.offsetHeight;
    let animating = false;
    let timer = 0;
    let queued = false;
    let raf = 0;
    let lockFrom = 0;
    let easing = false;

    const finish = () => {
      animating = false;
      easing = false;
      window.clearTimeout(timer);
      el.style.height = "";
      el.style.overflowY = "";
      el.style.transition = "";
      settled = el.offsetHeight;
    };
    const reduced = () =>
      document.documentElement.dataset.motion === "reduced" ||
      window.matchMedia?.("(prefers-reduced-motion: reduce)").matches;

    const onSwitch = () => {
      queued = false;
      if (reduced()) return;
      const from = animating ? el.offsetHeight : settled;
      // Hold the old height now; the new panel's content can arrive a
      // commit after its `data-state` flips (Radix Presence), so measure
      // the target on the next frame and ease to it.
      animating = true;
      easing = false;
      el.style.transition = "none";
      el.style.overflowY = "clip";
      el.style.height = `${from}px`;
      window.clearTimeout(timer);
      cancelAnimationFrame(raf);
      lockFrom = from;
      raf = requestAnimationFrame(() => {
        // Nothing to ease yet: keep holding briefly in case the panel's
        // content lands a moment later (the ResizeObserver starts it).
        if (Math.abs(natural() - from) < 4) {
          timer = window.setTimeout(finish, LATE_CONTENT_MS);
          return;
        }
        easeTo(natural());
      });
    };
    const easeTo = (to: number) => {
      window.clearTimeout(timer);
      void el.offsetHeight; // commit the start height before transitioning
      easing = true;
      el.style.transition = `height ${PANEL_HEIGHT_MS}ms cubic-bezier(0.22, 1, 0.36, 1)`;
      el.style.height = `${to}px`;
      timer = window.setTimeout(finish, PANEL_HEIGHT_MS + 80);
    };
    const schedule = () => {
      if (queued) return;
      queued = true;
      queueMicrotask(onSwitch);
    };

    const mo = new MutationObserver((records) => {
      if (
        records.some((r) =>
          r.type === "childList"
            ? r.target === el
            : (r.target as Element).parentElement === el,
        )
      )
        schedule();
    });
    mo.observe(el, {
      childList: true,
      subtree: true,
      attributes: true,
      attributeFilter: ["data-state"],
    });
    // Between switches remember the settled height; while easing, follow
    // the new panel if its content changes size (lazy data arriving).
    const ro = new ResizeObserver(() => {
      if (!animating) {
        settled = el.offsetHeight;
        return;
      }
      const to = natural();
      if (easing) el.style.height = `${to}px`;
      else if (Math.abs(to - lockFrom) >= 4) easeTo(to);
    });
    ro.observe(el);
    const observeChildren = () => {
      for (const c of el.children) ro.observe(c);
    };
    observeChildren();
    const mo2 = new MutationObserver(observeChildren);
    mo2.observe(el, { childList: true });
    return () => {
      mo.disconnect();
      mo2.disconnect();
      ro.disconnect();
      window.clearTimeout(timer);
      cancelAnimationFrame(raf);
    };
  }, [ref]);
}

export function StableTabsPanel({
  value,
  children,
  className,
}: {
  value: string;
  children: React.ReactNode;
  className?: string;
}) {
  return (
    <TabsContent
      forceMount
      value={value}
      className={cn(
        STACKED_PANEL_CLASS,
        INACTIVE_STABLE_PANEL_CLASS,
        className,
      )}
    >
      {children}
    </TabsContent>
  );
}

export function StackedTabsPanel({
  value,
  children,
  className,
}: {
  value: string;
  children: React.ReactNode;
  className?: string;
}) {
  return (
    <TabsContent value={value} className={cn(STACKED_PANEL_CLASS, className)}>
      {children}
    </TabsContent>
  );
}
