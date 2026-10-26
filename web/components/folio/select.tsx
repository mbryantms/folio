"use client";

/* Folio wrapper over the vendored shadcn file (components/ui is pristine):
 * popper content defaults to the safe-area-aware collisionPadding so a
 * menu opened near a screen edge stays clear of the status bar, the
 * rounded corners and the bottom tab bar in the installed app. App code
 * imports from here, never from components/ui (ESLint enforces it). */
import * as React from "react";

import { safeAreaCollisionPadding } from "@/components/folio/collision-padding";
import {
  SelectContent as UiSelectContent,
  SelectTrigger as UiSelectTrigger,
} from "@/components/ui/select";
import { cn } from "@/lib/utils";

export * from "@/components/ui/select";

/* Upstream triggers are `w-fit`; Folio's forms and toolbars were built on
 * the earlier `w-full` default, so keep that here (a caller's own width
 * class still wins through `cn`). */
export function SelectTrigger({
  className,
  ...props
}: React.ComponentProps<typeof UiSelectTrigger>) {
  return <UiSelectTrigger className={cn("w-full", className)} {...props} />;
}

export function SelectContent({
  collisionPadding,
  position = "popper",
  ...props
}: React.ComponentProps<typeof UiSelectContent>) {
  const pad = React.useMemo(
    () => collisionPadding ?? safeAreaCollisionPadding(),
    [collisionPadding],
  );
  return (
    <UiSelectContent
      position={position}
      collisionPadding={position === "popper" ? pad : undefined}
      {...props}
    />
  );
}
