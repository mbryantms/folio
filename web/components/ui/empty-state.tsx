import * as React from "react";

import { cn } from "@/lib/utils";

const SIZES = {
  /** Full-page / main-content empty state. */
  default: "px-6 py-16",
  /** Inline panel / list (e.g. a filtered grid with no matches). */
  sm: "p-8",
  /** Sidebar rail / compact card. */
  rail: "px-4 py-8",
} as const;

export interface EmptyStateProps {
  icon?: React.ComponentType<{ className?: string }>;
  /** Optional heading; omit for a message-only empty state. */
  title?: string;
  description?: string;
  action?: React.ReactNode;
  size?: keyof typeof SIZES;
  className?: string;
}

/**
 * Shared empty-state panel — promoted out of `admin/` and given `size` variants
 * so the five hand-rolled library empty states (F3) can share one look.
 */
export function EmptyState({
  icon: Icon,
  title,
  description,
  action,
  size = "default",
  className,
}: EmptyStateProps) {
  return (
    <div
      className={cn(
        "border-border bg-card/40 flex flex-col items-center justify-center gap-3 rounded-lg border border-dashed text-center",
        SIZES[size],
        className,
      )}
    >
      {Icon ? (
        <div className="bg-secondary text-muted-foreground rounded-full p-3">
          <Icon className="h-5 w-5" />
        </div>
      ) : null}
      {title || description ? (
        <div className="space-y-1">
          {title ? (
            <h2 className="text-foreground text-base font-medium">{title}</h2>
          ) : null}
          {description ? (
            <p className="text-muted-foreground text-sm">{description}</p>
          ) : null}
        </div>
      ) : null}
      {action ? <div className="pt-2">{action}</div> : null}
    </div>
  );
}
