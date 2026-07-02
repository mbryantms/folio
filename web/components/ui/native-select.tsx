"use client";

import * as React from "react";

import { cn } from "@/lib/utils";

export interface NativeSelectOption {
  value: string;
  label: string;
}

export interface NativeSelectProps extends Omit<
  React.SelectHTMLAttributes<HTMLSelectElement>,
  "onChange" | "size"
> {
  options: NativeSelectOption[];
  /** Called with the selected value (not the raw event). */
  onChange?: (next: string) => void;
  /** `default` = full-width form control; `sm` = compact inline filter. */
  size?: "default" | "sm";
}

const SIZES = {
  default: "h-9 w-full px-3 py-1 text-sm",
  sm: "h-8 w-auto px-2 text-xs",
} as const;

/**
 * The styled native `<select>` that was copy-pasted verbatim in SeriesEditDrawer
 * and IssueActions, plus the home for the divergent raw `<select>`s the audit
 * flagged (F3) — `size="sm"` is the compact inline-filter variant. Forwards
 * standard select props (`id`, `value`, `disabled`, …); `onChange` receives the
 * value string.
 */
export function NativeSelect({
  options,
  onChange,
  size = "default",
  className,
  ...props
}: NativeSelectProps) {
  return (
    <select
      className={cn(
        "border-input bg-background focus-visible:ring-ring flex rounded-md border shadow-sm transition-colors focus-visible:ring-1 focus-visible:outline-none disabled:cursor-not-allowed disabled:opacity-50",
        SIZES[size],
        className,
      )}
      onChange={onChange ? (e) => onChange(e.target.value) : undefined}
      {...props}
    >
      {options.map((o) => (
        <option key={o.value} value={o.value}>
          {o.label}
        </option>
      ))}
    </select>
  );
}
