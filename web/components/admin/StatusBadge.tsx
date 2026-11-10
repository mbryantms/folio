import * as React from "react";

import { Badge } from "@/components/ui/badge";
import { statusTone, type StatusTone } from "@/lib/ui/status-tone";
import { cn } from "@/lib/utils";

/**
 * Status badge for admin tables: the outline badge with a subtle tone
 * surface (`statusTone`). Pick the tone with the rules in
 * `./status-tones.ts`, never a palette class. `data-tone` names the tone
 * for tests and for any `data-*` style rule.
 */
export function StatusBadge({
  tone,
  className,
  children,
  ...props
}: Omit<React.ComponentProps<typeof Badge>, "variant"> & {
  tone: StatusTone;
}) {
  return (
    <Badge
      variant="outline"
      data-tone={tone}
      className={cn(statusTone(tone), className)}
      {...props}
    >
      {children}
    </Badge>
  );
}
