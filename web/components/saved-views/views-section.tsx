"use client";

import * as React from "react";
import type { LucideIcon } from "lucide-react";

/** One section of the unified `/views` index (Filter views · Reading lists ·
 *  Collections). Shared by `ViewsIndex` and the embedded `CollectionsIndex`
 *  so all three headers line up. `id` is the anchor target — e.g.
 *  `/collections` redirects to `/views#collections`. */
export function ViewsSection({
  id,
  icon: Icon,
  title,
  blurb,
  action,
  children,
}: {
  id: string;
  icon: LucideIcon;
  title: string;
  blurb: string;
  action?: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <section id={id} className="scroll-mt-24 space-y-3">
      <div className="border-border/60 flex flex-wrap items-center justify-between gap-2 border-b pb-2">
        <div className="flex min-w-0 items-center gap-2">
          <Icon
            className="text-muted-foreground h-5 w-5 shrink-0"
            aria-hidden="true"
          />
          <h2 className="text-lg font-semibold">{title}</h2>
          <span className="text-muted-foreground hidden text-sm sm:inline">
            · {blurb}
          </span>
        </div>
        {action}
      </div>
      {children}
    </section>
  );
}
