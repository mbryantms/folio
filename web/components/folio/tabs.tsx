"use client";

/* Folio wrapper over the vendored shadcn file (components/ui is pristine).
 * `TabsList` defaults to upstream's `line` variant — section navigation
 * (series, issue, admin pages) is a full-width strip on a hairline with
 * the accent underline from `globals.css`; mode pickers (sign-in, CBL
 * import source, scan-run filter) pass `variant="default"` for the
 * segmented pill, whose selected segment is solid primary there. It
 * scrolls horizontally when the triggers overflow (phones, the series
 * page's seven tabs), hides its scrollbar, and fades the edge that has
 * more tabs behind it (`data-fade`). It scrolls on the x axis only: with
 * `overflow-x-auto` alone the 44px tap areas poking past the 36px strip
 * gave it a vertical scroll range that iOS panned (2026-10-24). App code
 * imports from here, never from components/ui (ESLint enforces it). */
import * as React from "react";

import { Tabs as UiTabs, TabsList as UiTabsList } from "@/components/ui/tabs";
import {
  SLIDE_TRANSITION,
  useSlidingIndicator,
} from "@/lib/ui/use-sliding-indicator";
import { cn } from "@/lib/utils";

// Re-exported explicitly: `Tabs` and `TabsList` are Folio wrappers below.
export {
  TabsContent,
  TabsTrigger,
  tabsListVariants,
} from "@/components/ui/tabs";

type EdgeFade = "" | "left" | "right" | "both";

function useEdgeFade<T extends HTMLElement>(): [
  React.RefObject<T | null>,
  EdgeFade,
] {
  const ref = React.useRef<T | null>(null);
  const [fade, setFade] = React.useState<EdgeFade>("");
  React.useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const update = () => {
      const left = el.scrollLeft > 1;
      const right = el.scrollLeft + el.clientWidth < el.scrollWidth - 1;
      setFade(left && right ? "both" : left ? "left" : right ? "right" : "");
    };
    update();
    el.addEventListener("scroll", update, { passive: true });
    window.addEventListener("resize", update);
    const ro =
      typeof ResizeObserver !== "undefined" ? new ResizeObserver(update) : null;
    ro?.observe(el);
    return () => {
      el.removeEventListener("scroll", update);
      window.removeEventListener("resize", update);
      ro?.disconnect();
    };
  }, []);
  return [ref, fade];
}

/** The line strip's accent underline as one element that glides to the
 *  active tab (folio-flair), instead of each trigger fading its own
 *  `::after` in and out. While it is shown the strip carries
 *  `data-indicator`, which hides the per-trigger underline (globals.css).
 *  The measuring lives in `useSlidingIndicator`, shared with the link
 *  navs and segmented controls. */
const TAB_INDICATOR = {
  active: ':scope > [role="tab"][data-state="active"]',
  items: ':scope > [role="tab"]',
  attributes: ["data-state"],
};

export function TabsList({
  className,
  ref,
  variant = "line",
  children,
  ...props
}: React.ComponentProps<typeof UiTabsList>) {
  const [inner, fade] = useEdgeFade<HTMLDivElement>();
  React.useImperativeHandle(ref, () => inner.current as HTMLDivElement);
  const indicator = useSlidingIndicator(inner, {
    ...TAB_INDICATOR,
    enabled: variant === "line",
  });
  return (
    <UiTabsList
      ref={inner}
      variant={variant}
      data-fade={fade || undefined}
      data-indicator={indicator ? "" : undefined}
      className={cn(
        "justify-[safe_center] flex w-full max-w-full overflow-x-auto overflow-y-hidden md:inline-flex md:w-fit",
        "[scrollbar-width:none] [&::-webkit-scrollbar]:hidden [&>*]:shrink-0",
        "data-[fade=both]:mask-x-from-[calc(100%-2rem)] data-[fade=left]:mask-l-from-[calc(100%-2rem)] data-[fade=right]:mask-r-from-[calc(100%-2rem)]",
        // Navigation strips span their column and start at its left edge.
        variant === "line" &&
          "border-border justify-start border-b md:flex md:w-full",
        variant === "line" && "relative",
        className,
      )}
      {...props}
    >
      {children}
      {indicator ? (
        <span
          aria-hidden
          data-slot="tabs-indicator"
          className={cn(
            "bg-primary pointer-events-none absolute top-0 left-0 h-0.5 rounded-full",
            indicator.animate && SLIDE_TRANSITION,
          )}
          style={{
            width: indicator.width,
            // Same spot as the trigger's own underline: 2px tall, its
            // bottom edge 4px below the trigger (inside the strip's
            // padding).
            translate: `${indicator.left}px ${indicator.top + indicator.height + 2}px`,
          }}
        />
      ) : null}
    </UiTabsList>
  );
}

/**
 * Tabs root with a panel entrance (folio-flair): when the tab changes,
 * the incoming panel fades in with a short slide from the side the new
 * tab sits on (`data-tab-motion="fwd" | "back"` on the root; keyframes in
 * globals.css). The first render does not animate. Reduced motion: the
 * global reset makes it an instant swap.
 */
export function Tabs({
  ref,
  value,
  defaultValue,
  onValueChange,
  ...props
}: React.ComponentProps<typeof UiTabs>) {
  const root = React.useRef<HTMLDivElement | null>(null);
  React.useImperativeHandle(ref, () => root.current as HTMLDivElement);
  // The tab we are leaving. Read from our own record, not the DOM:
  // uncontrolled Radix Tabs report the change after the new tab is
  // already active.
  const prev = React.useRef(value ?? defaultValue);
  React.useEffect(() => {
    if (value !== undefined) prev.current = value;
  }, [value]);
  const handle = (v: string) => {
    const el = root.current;
    if (el && prev.current !== v) {
      const ids = [...el.querySelectorAll<HTMLElement>('[role="tab"]')]
        .filter((t) => t.closest('[data-slot="tabs"]') === el)
        .map((t) => t.id);
      const at = (x: string | undefined) =>
        x === undefined
          ? -1
          : ids.findIndex((id) => id.endsWith(`-trigger-${x}`));
      const from = at(prev.current);
      const to = at(v);
      if (from >= 0 && to >= 0)
        el.dataset.tabMotion = to > from ? "fwd" : "back";
    }
    prev.current = v;
    onValueChange?.(v);
  };
  return (
    <UiTabs
      ref={root}
      value={value}
      defaultValue={defaultValue}
      onValueChange={handle}
      {...props}
    />
  );
}
