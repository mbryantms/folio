"use client";

/* Folio wrapper over the vendored shadcn file (components/ui is pristine):
 * popper content defaults to the safe-area-aware collisionPadding so a
 * popover opened near a screen edge stays clear of the status bar, the
 * rounded corners and the bottom tab bar in the installed app, and a tall
 * popover scrolls instead of running off-screen. App code imports from
 * here, never from components/ui (ESLint enforces it). */
import * as React from "react";
import { Popover as PopoverPrimitive } from "radix-ui";

import { safeAreaCollisionPadding } from "@/components/folio/collision-padding";
import { PopoverContent as UiPopoverContent } from "@/components/ui/popover";
import { cn } from "@/lib/utils";

export * from "@/components/ui/popover";

/* A popover inside a modal Dialog / Sheet: the dialog's focus trap fights
 * a popover that portals to <body> (the symptom was "popover opens but
 * the search input inside it doesn't accept focus"), and Radix's `modal`
 * popover mode is no answer — it aria-hides everything outside the
 * popover, the hosting dialog included, which breaks axe and any
 * role-based locator on the dialog. Wrap the dialog body in
 * `<PopoverPortalContainer value={dialogContentEl}>` and every descendant
 * Popover portals into that element instead.
 *
 * The vendored `PopoverContent` owns its portal, so inside a container
 * this renders the Radix content itself. Its class string mirrors the
 * vendored one; the weekly refresh diff on components/ui/popover.tsx is
 * the cue to re-sync it. */
const PopoverPortalContext = React.createContext<HTMLElement | null>(null);

export const PopoverPortalContainer = PopoverPortalContext.Provider;

const SCROLL_CAP =
  "max-h-(--radix-popover-content-available-height) overflow-y-auto";

/* Mirror of components/ui/popover.tsx `PopoverContent` (radix-vega). */
const UPSTREAM_CONTENT_CLASS =
  "z-50 flex w-72 origin-(--radix-popover-content-transform-origin) flex-col gap-4 rounded-md bg-popover p-4 text-sm text-popover-foreground shadow-md ring-1 ring-foreground/10 outline-hidden duration-100 data-[side=bottom]:slide-in-from-top-2 data-[side=left]:slide-in-from-right-2 data-[side=right]:slide-in-from-left-2 data-[side=top]:slide-in-from-bottom-2 data-open:animate-in data-open:fade-in-0 data-open:zoom-in-95 data-closed:animate-out data-closed:fade-out-0 data-closed:zoom-out-95";

export function PopoverContent({
  collisionPadding,
  className,
  align = "center",
  sideOffset = 4,
  ...props
}: React.ComponentProps<typeof UiPopoverContent>) {
  const container = React.useContext(PopoverPortalContext);
  const pad = React.useMemo(
    () => collisionPadding ?? safeAreaCollisionPadding(),
    [collisionPadding],
  );
  if (container) {
    return (
      <PopoverPrimitive.Portal container={container}>
        <PopoverPrimitive.Content
          data-slot="popover-content"
          align={align}
          sideOffset={sideOffset}
          collisionPadding={pad}
          className={cn(UPSTREAM_CONTENT_CLASS, SCROLL_CAP, className)}
          {...props}
        />
      </PopoverPrimitive.Portal>
    );
  }
  return (
    <UiPopoverContent
      align={align}
      sideOffset={sideOffset}
      collisionPadding={pad}
      className={cn(SCROLL_CAP, className)}
      {...props}
    />
  );
}
