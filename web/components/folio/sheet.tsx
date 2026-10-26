"use client";

/* Folio wrapper over the vendored shadcn Sheet (components/ui is pristine).
 *
 * - `size`: the upstream file caps every left/right sheet at `max-w-sm`
 *   with a `data-[side=…]:sm:max-w-sm` class that outranks a caller's plain
 *   `sm:max-w-xl` (attribute selector vs class), which is how the CBL
 *   matching sheet and the edit sheets all shrank to one size
 *   (2026-10-26). The width is now a `--sheet-max-w` variable the
 *   globals.css rule reads at `sm+`, so the caller's choice wins without
 *   fighting the utility.
 * - Padding: upstream pads the header and footer (`p-4`) and not the
 *   body; Folio's sheets were written against a padded body (`p-6`) and
 *   unpadded header / footer that carry their own classes, so restore
 *   that shape. All three go through `cn`, so a caller's `p-0` / `px-6`
 *   still wins.
 * App code imports from here, never from components/ui (ESLint enforces
 * it). */
import * as React from "react";

import {
  SheetContent as UiSheetContent,
  SheetFooter as UiSheetFooter,
  SheetHeader as UiSheetHeader,
} from "@/components/ui/sheet";
import { cn } from "@/lib/utils";

export * from "@/components/ui/sheet";

export type SheetSize =
  "sm" | "md" | "lg" | "xl" | "2xl" | "3xl" | "4xl" | "5xl";

const SIZE_VARS: Record<SheetSize, string> = {
  sm: "var(--container-sm)",
  md: "var(--container-md)",
  lg: "var(--container-lg)",
  xl: "var(--container-xl)",
  "2xl": "var(--container-2xl)",
  "3xl": "var(--container-3xl)",
  "4xl": "var(--container-4xl)",
  "5xl": "var(--container-5xl)",
};

/* Sizes from 2xl up hold dense content (the CBL match table, the
 * shortcuts grid) that does not fit the 75% phone drawer, so they take the
 * whole viewport below `sm`; smaller sheets keep the drawer with the page
 * peeking behind. */
const FULL_WIDTH_ON_PHONES: ReadonlySet<SheetSize> = new Set([
  "2xl",
  "3xl",
  "4xl",
  "5xl",
]);

export function SheetContent({
  size = "sm",
  className,
  style,
  ...props
}: React.ComponentProps<typeof UiSheetContent> & { size?: SheetSize }) {
  return (
    <UiSheetContent
      className={cn(
        "p-6",
        FULL_WIDTH_ON_PHONES.has(size) &&
          "max-sm:data-[side=left]:w-full max-sm:data-[side=right]:w-full",
        className,
      )}
      style={
        { "--sheet-max-w": SIZE_VARS[size], ...style } as React.CSSProperties
      }
      {...props}
    />
  );
}

export function SheetHeader({
  className,
  ...props
}: React.ComponentProps<typeof UiSheetHeader>) {
  return <UiSheetHeader className={cn("p-0", className)} {...props} />;
}

export function SheetFooter({
  className,
  ...props
}: React.ComponentProps<typeof UiSheetFooter>) {
  return <UiSheetFooter className={cn("p-0", className)} {...props} />;
}
