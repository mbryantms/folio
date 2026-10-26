"use client";

/* Folio wrapper over the vendored shadcn file (components/ui is pristine).
 * The upstream `ScrollArea` owns its viewport, so this one composes the
 * Radix primitives itself and reuses the upstream `ScrollBar` (the styled
 * part): callers can cap the *viewport* (`viewportClassName`, where a
 * `max-h-*` must sit for content-sized areas), make it keyboard-scrollable
 * (`viewportProps={{ tabIndex: 0 }}`, axe scrollable-region-focusable),
 * and add a horizontal bar for content wider than a phone. App code
 * imports from here, never from components/ui (ESLint enforces it). */
import * as React from "react";
import { ScrollArea as ScrollAreaPrimitive } from "radix-ui";

import { ScrollBar } from "@/components/ui/scroll-area";
import { cn } from "@/lib/utils";

export { ScrollBar };

export function ScrollArea({
  className,
  viewportClassName,
  viewportProps,
  horizontal = false,
  children,
  ...props
}: React.ComponentProps<typeof ScrollAreaPrimitive.Root> & {
  viewportClassName?: string;
  viewportProps?: Omit<
    React.ComponentProps<typeof ScrollAreaPrimitive.Viewport>,
    "className" | "children"
  >;
  horizontal?: boolean;
}) {
  return (
    <ScrollAreaPrimitive.Root
      data-slot="scroll-area"
      className={cn("relative", className)}
      {...props}
    >
      <ScrollAreaPrimitive.Viewport
        data-slot="scroll-area-viewport"
        {...viewportProps}
        className={cn(
          "focus-visible:ring-ring/50 size-full rounded-[inherit] transition-[color,box-shadow] outline-none focus-visible:ring-[3px] focus-visible:outline-1",
          viewportClassName,
        )}
      >
        {children}
      </ScrollAreaPrimitive.Viewport>
      <ScrollBar />
      {horizontal ? <ScrollBar orientation="horizontal" /> : null}
      <ScrollAreaPrimitive.Corner />
    </ScrollAreaPrimitive.Root>
  );
}
