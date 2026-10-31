import * as React from "react";

import { cn } from "@/lib/utils";

/**
 * Unified filter / toggle pill for list-filter rows (bookmarks,
 * findings, admin health + users, activity feed, …). Standardizes the
 * shape (rounded-full, compact) and the coloring — primary-accent tint
 * when active, muted otherwise — so every filter row matches. Forwards
 * all `<button>` props, so callers attach their own `onClick`,
 * `disabled`, etc.
 *
 * `aria-pressed` is emitted from `active` by default — no call site
 * ever attached it by hand, so toggle state was color-only for AT
 * (WCAG 4.1.2). A pill given another role (`role="tab"` +
 * `aria-selected`, `role="radio"` + `aria-checked`) carries that role's
 * state attribute instead; `aria-pressed` is only valid on a button.
 *
 * `count` renders an optional trailing tally (tag chips, status counts)
 * with active-aware coloring. Labels render in their natural case; pass
 * `className="capitalize"` for call sites whose option values are
 * lowercase keys.
 */
export interface FilterPillProps extends React.ButtonHTMLAttributes<HTMLButtonElement> {
  active: boolean;
  count?: number;
}

export function FilterPill({
  active,
  count,
  className,
  children,
  type,
  role,
  ...props
}: FilterPillProps) {
  const isToggleButton = role === undefined || role === "button";
  return (
    <button
      type={type ?? "button"}
      role={role}
      data-active={active || undefined}
      aria-pressed={isToggleButton ? active : undefined}
      className={cn(
        "folio-tap focus-visible:ring-ring inline-flex items-center gap-1 rounded-full border px-2.5 py-0.5 text-xs transition-colors focus-visible:ring-2 focus-visible:outline-none",
        active
          ? "border-primary bg-primary/10 text-primary"
          : "border-border text-muted-foreground hover:text-foreground",
        className,
      )}
      {...props}
    >
      {children}
      {count !== undefined ? (
        <span
          className={cn(
            "tabular-nums",
            active ? "text-primary/70" : "text-muted-foreground",
          )}
        >
          {count}
        </span>
      ) : null}
    </button>
  );
}
