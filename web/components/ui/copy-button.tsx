"use client";

import * as React from "react";
import { Check, Copy } from "lucide-react";

import { Button, type ButtonProps } from "@/components/ui/button";
import { cn } from "@/lib/utils";

/**
 * Copy-to-clipboard logic, consolidated from the four hand-rolled copies the
 * frontend audit flagged (F3). Returns `copied` (true for `resetMs` after a
 * successful copy, for a check-mark affordance) and `copy(text)` which resolves
 * `false` when the clipboard API is missing or the write throws — callers that
 * want a failure toast can branch on it.
 */
export function useCopyToClipboard(resetMs = 1500) {
  const [copied, setCopied] = React.useState(false);
  const timer = React.useRef<ReturnType<typeof setTimeout> | undefined>(
    undefined,
  );
  React.useEffect(() => () => clearTimeout(timer.current), []);

  const copy = React.useCallback(
    async (text: string): Promise<boolean> => {
      if (!navigator.clipboard) return false;
      try {
        await navigator.clipboard.writeText(text);
        setCopied(true);
        clearTimeout(timer.current);
        timer.current = setTimeout(() => setCopied(false), resetMs);
        return true;
      } catch {
        return false;
      }
    },
    [resetMs],
  );

  return { copied, copy };
}

export interface CopyButtonProps
  extends Omit<ButtonProps, "value" | "onClick" | "children"> {
  /** Text written to the clipboard on click. */
  value: string;
  /** Label shown next to the copy icon; omit for an icon-only button. */
  label?: string;
  copiedLabel?: string;
}

/**
 * Standard "Copy" button: swaps to a check mark for a moment after copying.
 * Icon-only when `label` is omitted (pass an `aria-label` in that case).
 */
export function CopyButton({
  value,
  label = "Copy",
  copiedLabel = "Copied",
  variant = "outline",
  size,
  className,
  ...props
}: CopyButtonProps) {
  const { copied, copy } = useCopyToClipboard();
  const iconOnly = !label;
  const Icon = copied ? Check : Copy;
  return (
    <Button
      type="button"
      variant={variant}
      size={size ?? (iconOnly ? "icon" : "sm")}
      onClick={() => void copy(value)}
      className={className}
      {...props}
    >
      <Icon className={cn(iconOnly ? "size-4" : "size-3.5")} />
      {iconOnly ? null : copied ? copiedLabel : label}
    </Button>
  );
}
