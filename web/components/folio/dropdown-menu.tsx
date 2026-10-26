"use client";

/* Folio wrapper over the vendored shadcn file (components/ui is pristine):
 * popper content defaults to the safe-area-aware collisionPadding so a
 * menu opened near a screen edge stays clear of the status bar, the
 * rounded corners and the bottom tab bar in the installed app. App code
 * imports from here, never from components/ui (ESLint enforces it). */
import * as React from "react";

import { safeAreaCollisionPadding } from "@/components/folio/collision-padding";
import {
  DropdownMenuContent as UiDropdownMenuContent,
  DropdownMenuSubContent as UiDropdownMenuSubContent,
} from "@/components/ui/dropdown-menu";

export * from "@/components/ui/dropdown-menu";

export function DropdownMenuContent({
  collisionPadding,
  ...props
}: React.ComponentProps<typeof UiDropdownMenuContent>) {
  const pad = React.useMemo(
    () => collisionPadding ?? safeAreaCollisionPadding(),
    [collisionPadding],
  );
  return <UiDropdownMenuContent collisionPadding={pad} {...props} />;
}

export function DropdownMenuSubContent({
  collisionPadding,
  ...props
}: React.ComponentProps<typeof UiDropdownMenuSubContent>) {
  const pad = React.useMemo(
    () => collisionPadding ?? safeAreaCollisionPadding(),
    [collisionPadding],
  );
  return <UiDropdownMenuSubContent collisionPadding={pad} {...props} />;
}
