"use client";

import * as React from "react";
import { ChevronDown, ChevronRight } from "lucide-react";

import { cn } from "@/lib/utils";

/**
 * Collapsible accordion-style section for picker dialogs (Manage rails on a
 * page, Add to sidebar, …). Each section keeps its own open/closed state
 * with a chevron-toggled header showing the row count. Designed to drop
 * into long lists that would otherwise force the user to scroll past
 * sections they don't care about.
 *
 * `forceOpen` overrides the local toggle (used to auto-expand all
 * sections while a search query is active — collapsed sections would
 * hide search matches otherwise).
 */
export function CollapsiblePickerSection({
  label,
  count,
  defaultOpen = true,
  forceOpen,
  children,
}: {
  label: string;
  count: number;
  defaultOpen?: boolean;
  forceOpen?: boolean;
  children: React.ReactNode;
}) {
  const [open, setOpen] = React.useState(defaultOpen);
  const effectiveOpen = forceOpen ?? open;
  const headerId = React.useId();
  const panelId = React.useId();
  return (
    <div className="border-border/40 rounded-md border">
      <button
        type="button"
        id={headerId}
        aria-expanded={effectiveOpen}
        aria-controls={panelId}
        onClick={() => setOpen((v) => !v)}
        disabled={forceOpen !== undefined}
        className={cn(
          "hover:bg-secondary/40 flex w-full items-center gap-2 px-3 py-1.5 text-left transition-colors",
          forceOpen !== undefined && "cursor-default opacity-90",
        )}
      >
        {effectiveOpen ? (
          <ChevronDown className="text-muted-foreground h-3.5 w-3.5 shrink-0" />
        ) : (
          <ChevronRight className="text-muted-foreground h-3.5 w-3.5 shrink-0" />
        )}
        <span className="text-muted-foreground/80 flex-1 text-[10px] font-medium tracking-widest uppercase">
          {label}
        </span>
        <span className="text-muted-foreground/60 text-[11px] tabular-nums">
          {count}
        </span>
      </button>
      <div
        id={panelId}
        role="region"
        aria-labelledby={headerId}
        hidden={!effectiveOpen}
        className="px-1 pb-1"
      >
        {children}
      </div>
    </div>
  );
}
