"use client";

/* Folio wrapper over the vendored shadcn NativeSelect (components/ui is
 * pristine): the `options` list + string `onChange` the call sites use,
 * and full width by default (upstream's wrapper is `w-fit`). The
 * `<select>` keeps its shadcn size on every pointer; as a wide target it
 * needs no tap area (phone-layout rule 9). */
import * as React from "react";

import {
  NativeSelect as UiNativeSelect,
  NativeSelectOption,
} from "@/components/ui/native-select";
import { cn } from "@/lib/utils";

export {
  NativeSelectOptGroup,
  NativeSelectOption,
} from "@/components/ui/native-select";

export interface NativeSelectOptionItem {
  value: string;
  label: string;
}

export interface NativeSelectProps extends Omit<
  React.ComponentProps<typeof UiNativeSelect>,
  "onChange" | "children"
> {
  options: NativeSelectOptionItem[];
  onChange?: (next: string) => void;
}

export function NativeSelect({
  options,
  onChange,
  size = "default",
  className,
  ...props
}: NativeSelectProps) {
  return (
    <UiNativeSelect
      size={size}
      className={cn(size === "sm" ? "w-auto" : "w-full", className)}
      onChange={onChange ? (e) => onChange(e.target.value) : undefined}
      {...props}
    >
      {options.map((o) => (
        <NativeSelectOption key={o.value} value={o.value}>
          {o.label}
        </NativeSelectOption>
      ))}
    </UiNativeSelect>
  );
}
