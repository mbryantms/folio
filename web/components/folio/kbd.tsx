import * as React from "react";

import { cn } from "@/lib/utils";

const KBD_SIZES = {
  /** Inline keycap inside running text / menus (the dominant style). */
  default: "h-4 min-w-4 px-1 text-[10px]",
  /** Standalone shortcut keycap (shortcuts sheet). */
  md: "min-w-8 px-2 py-0.5 text-xs",
  /** Key-capture display (the rebind editor's big boxes). */
  lg: "min-w-8 px-3 py-2 text-base",
} as const;

export interface KbdProps extends React.HTMLAttributes<HTMLElement> {
  size?: keyof typeof KBD_SIZES;
}

/**
 * One keyboard-key chip, replacing the three divergent `<kbd>` styles the audit
 * flagged (F3) — including an off-theme neutral one. Theme-token colors; `size`
 * picks inline keycap / shortcut keycap / capture display.
 */
export function Kbd({ size = "default", className, ...props }: KbdProps) {
  return (
    <kbd
      className={cn(
        "border-border bg-muted text-foreground inline-flex items-center justify-center rounded border font-mono leading-none",
        KBD_SIZES[size],
        className,
      )}
      {...props}
    />
  );
}
