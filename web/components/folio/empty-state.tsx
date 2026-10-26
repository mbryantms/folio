/* Folio wrapper over the vendored shadcn Empty composition (components/ui
 * is pristine): the one-call `EmptyState` the list surfaces use, with the
 * three paddings they were built on. */
import * as React from "react";

import {
  Empty,
  EmptyContent,
  EmptyDescription,
  EmptyHeader,
  EmptyMedia,
  EmptyTitle,
} from "@/components/ui/empty";
import { cn } from "@/lib/utils";

const SIZES = {
  default: "px-6 py-16",
  sm: "p-8",
  rail: "px-4 py-8",
} as const;

export interface EmptyStateProps {
  icon?: React.ComponentType<{ className?: string }>;
  title?: string;
  description?: string;
  action?: React.ReactNode;
  size?: keyof typeof SIZES;
  className?: string;
}

export function EmptyState({
  icon: Icon,
  title,
  description,
  action,
  size = "default",
  className,
}: EmptyStateProps) {
  return (
    <Empty className={cn("bg-card/40 border", SIZES[size], className)}>
      {Icon || title || description ? (
        <EmptyHeader>
          {Icon ? (
            <EmptyMedia variant="icon">
              <Icon className="size-5" />
            </EmptyMedia>
          ) : null}
          {title ? (
            <EmptyTitle className="text-base">{title}</EmptyTitle>
          ) : null}
          {description ? (
            <EmptyDescription>{description}</EmptyDescription>
          ) : null}
        </EmptyHeader>
      ) : null}
      {action ? <EmptyContent>{action}</EmptyContent> : null}
    </Empty>
  );
}
