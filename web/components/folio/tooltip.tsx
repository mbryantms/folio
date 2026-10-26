"use client";

/* Folio wrapper over the vendored shadcn file (components/ui is pristine):
 * popper content defaults to the safe-area-aware collisionPadding so a
 * menu opened near a screen edge stays clear of the status bar, the
 * rounded corners and the bottom tab bar in the installed app. App code
 * imports from here, never from components/ui (ESLint enforces it). */
import * as React from "react";

import { safeAreaCollisionPadding } from "@/components/folio/collision-padding";
import {
  Tooltip as UiTooltip,
  TooltipContent as UiTooltipContent,
  TooltipProvider as UiTooltipProvider,
} from "@/components/ui/tooltip";

export * from "@/components/ui/tooltip";

/* Each tooltip brings its own provider (the pre-2026 shadcn shape). A
 * single app-wide provider in the root layout pulled Radix Tooltip +
 * Popper + floating-ui (~16 KB gzip) into every route's first-load chunk,
 * reader included (bundle gate 143.6 → 160.7 KB, 2026-10-26). Surfaces
 * that render many tooltips can still share one `TooltipProvider` above
 * them; the vendored SidebarProvider already does. */
export function Tooltip(props: React.ComponentProps<typeof UiTooltip>) {
  return (
    <UiTooltipProvider delayDuration={200}>
      <UiTooltip {...props} />
    </UiTooltipProvider>
  );
}

export function TooltipContent({
  collisionPadding,
  ...props
}: React.ComponentProps<typeof UiTooltipContent>) {
  const pad = React.useMemo(
    () => collisionPadding ?? safeAreaCollisionPadding(),
    [collisionPadding],
  );
  return <UiTooltipContent collisionPadding={pad} {...props} />;
}
