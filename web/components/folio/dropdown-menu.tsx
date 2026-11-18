"use client";

/* Folio wrapper over the vendored shadcn file (components/ui is pristine):
 * popper content defaults to the safe-area-aware collisionPadding so a
 * menu opened near a screen edge stays clear of the status bar, the
 * rounded corners and the bottom tab bar in the installed app, and sizes
 * to its labels (MENU_CONTENT_WIDTH). App code imports from here, never
 * from components/ui (ESLint enforces it). */
import * as React from "react";

import { safeAreaCollisionPadding } from "@/components/folio/collision-padding";
import {
  DropdownMenuContent as UiDropdownMenuContent,
  DropdownMenuSubContent as UiDropdownMenuSubContent,
} from "@/components/ui/dropdown-menu";
import { cn } from "@/lib/utils";

export * from "@/components/ui/dropdown-menu";

/**
 * Menu width standard (docs/dev/menus.md). The vendored default is the
 * trigger's width (min 128px), so a menu opened from a 36px kebab wrapped
 * "Fetch metadata for view" onto two lines. Instead every menu sizes to its
 * longest label, never narrower than 12rem (192px) and never wider than
 * 20rem (320px) or the room left on screen; past that a label wraps.
 * A caller's own `w-*` / `min-w-*` / `max-w-*` still wins (tailwind-merge),
 * for the few menus that carry description lines.
 */
export const MENU_CONTENT_WIDTH =
  "w-auto min-w-48 max-w-[min(20rem,var(--radix-dropdown-menu-content-available-width))]";

export function DropdownMenuContent({
  collisionPadding,
  className,
  ...props
}: React.ComponentProps<typeof UiDropdownMenuContent>) {
  const pad = React.useMemo(
    () => collisionPadding ?? safeAreaCollisionPadding(),
    [collisionPadding],
  );
  return (
    <UiDropdownMenuContent
      collisionPadding={pad}
      className={cn(MENU_CONTENT_WIDTH, className)}
      {...props}
    />
  );
}

export function DropdownMenuSubContent({
  collisionPadding,
  className,
  ...props
}: React.ComponentProps<typeof UiDropdownMenuSubContent>) {
  const pad = React.useMemo(
    () => collisionPadding ?? safeAreaCollisionPadding(),
    [collisionPadding],
  );
  return (
    <UiDropdownMenuSubContent
      collisionPadding={pad}
      className={cn(MENU_CONTENT_WIDTH, className)}
      {...props}
    />
  );
}
