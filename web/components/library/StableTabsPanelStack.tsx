"use client";

import * as React from "react";

import { TabsContent } from "@/components/ui/tabs";
import { cn } from "@/lib/utils";

const STACKED_PANEL_CLASS = "col-start-1 row-start-1 pt-6";
const INACTIVE_STABLE_PANEL_CLASS =
  "data-[state=inactive]:pointer-events-none data-[state=inactive]:invisible";

export function StableTabsPanelStack({
  children,
  className,
}: {
  children: React.ReactNode;
  className?: string;
}) {
  // `minmax(0, 1fr)`: the one column is the container's width, never the
  // content's — a horizontal rail inside a panel (Related tab, WP-7.7)
  // would otherwise widen the implicit `auto` column and the page.
  return (
    <div className={cn("grid grid-cols-[minmax(0,1fr)]", className)}>
      {children}
    </div>
  );
}

export function StableTabsPanel({
  value,
  children,
  className,
}: {
  value: string;
  children: React.ReactNode;
  className?: string;
}) {
  return (
    <TabsContent
      forceMount
      value={value}
      className={cn(
        STACKED_PANEL_CLASS,
        INACTIVE_STABLE_PANEL_CLASS,
        className,
      )}
    >
      {children}
    </TabsContent>
  );
}

export function StackedTabsPanel({
  value,
  children,
  className,
}: {
  value: string;
  children: React.ReactNode;
  className?: string;
}) {
  return (
    <TabsContent value={value} className={cn(STACKED_PANEL_CLASS, className)}>
      {children}
    </TabsContent>
  );
}
