/* Folio wrapper over the vendored shadcn Kbd (components/ui is pristine):
 * the three keycap sizes Folio uses on top of the upstream default. */
import * as React from "react";

import { Kbd as UiKbd } from "@/components/ui/kbd";
import { cn } from "@/lib/utils";

export { KbdGroup } from "@/components/ui/kbd";

const KBD_SIZES = {
  /** Inline keycap inside running text / menus (the dominant style). */
  default: "h-4 min-w-4 px-1 text-[10px]",
  /** Standalone shortcut keycap (shortcuts sheet). */
  md: "h-auto min-w-8 px-2 py-0.5 text-xs",
  /** Key-capture display (the rebind editor's big boxes). */
  lg: "h-auto min-w-8 px-3 py-2 text-base",
} as const;

export interface KbdProps extends React.ComponentProps<typeof UiKbd> {
  size?: keyof typeof KBD_SIZES;
}

export function Kbd({ size = "default", className, ...props }: KbdProps) {
  return (
    <UiKbd
      className={cn(
        "border-border text-foreground pointer-events-auto rounded border font-mono",
        KBD_SIZES[size],
        className,
      )}
      {...props}
    />
  );
}
