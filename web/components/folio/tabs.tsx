"use client";

/* Folio wrapper over the vendored shadcn file (components/ui is pristine).
 * `TabsList` scrolls horizontally when the triggers overflow (phones, the
 * series page's seven tabs), hides its scrollbar, and fades the edge that
 * has more tabs behind it (`data-fade`). It scrolls on the x axis only:
 * with `overflow-x-auto` alone the 44px tap areas poking past the 36px
 * strip gave it a vertical scroll range that iOS panned (2026-10-24). App
 * code imports from here, never from components/ui (ESLint enforces it). */
import * as React from "react";

import { TabsList as UiTabsList } from "@/components/ui/tabs";
import { cn } from "@/lib/utils";

export * from "@/components/ui/tabs";

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

export function TabsList({
  className,
  ref,
  ...props
}: React.ComponentProps<typeof UiTabsList>) {
  const [inner, fade] = useEdgeFade<HTMLDivElement>();
  React.useImperativeHandle(ref, () => inner.current as HTMLDivElement);
  return (
    <UiTabsList
      ref={inner}
      data-fade={fade || undefined}
      className={cn(
        "justify-[safe_center] flex w-full max-w-full overflow-x-auto overflow-y-hidden md:inline-flex md:w-fit",
        "[scrollbar-width:none] [&::-webkit-scrollbar]:hidden [&>*]:shrink-0",
        "data-[fade=both]:mask-x-from-[calc(100%-2rem)] data-[fade=left]:mask-l-from-[calc(100%-2rem)] data-[fade=right]:mask-r-from-[calc(100%-2rem)]",
        className,
      )}
      {...props}
    />
  );
}
