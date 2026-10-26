"use client";

/* Folio wrapper over the vendored shadcn file (components/ui is pristine):
 * popper content defaults to the safe-area-aware collisionPadding so a
 * menu opened near a screen edge stays clear of the status bar, the
 * rounded corners and the bottom tab bar in the installed app. App code
 * imports from here, never from components/ui (ESLint enforces it). */
import * as React from "react";

import { safeAreaCollisionPadding } from "@/components/folio/collision-padding";
import { HoverCardContent as UiHoverCardContent } from "@/components/ui/hover-card";

export * from "@/components/ui/hover-card";

export function HoverCardContent({
  collisionPadding,
  ...props
}: React.ComponentProps<typeof UiHoverCardContent>) {
  const pad = React.useMemo(
    () => collisionPadding ?? safeAreaCollisionPadding(),
    [collisionPadding],
  );
  return <UiHoverCardContent collisionPadding={pad} {...props} />;
}
