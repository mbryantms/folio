"use client";

import type { Op } from "@/lib/api/types";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";

import { MultiSelectEditor } from "./MultiSelectEditor";

export type EnumEditorProps = {
  op: Op;
  value: unknown;
  values: readonly string[];
  /** Optional display-label map per wire value. Bare values render
   *  unchanged when absent. */
  labels?: Readonly<Record<string, string>>;
  onChange: (value: string | string[]) => void;
};

/** `is/is_not` are scalar; `in/not_in` accept a multi-select. We
 *  fold the multi-select form into the existing MultiSelectEditor with
 *  a fixed option list (no remote fetch). */
export function EnumEditor({
  op,
  value,
  values,
  labels,
  onChange,
}: EnumEditorProps) {
  const labelFor = (v: string) => labels?.[v] ?? v;
  if (op === "in" || op === "not_in") {
    return (
      <MultiSelectEditor
        value={value}
        onChange={(next) => onChange(next as string[])}
        staticOptions={values}
        optionLabel={labelFor}
      />
    );
  }
  const current = typeof value === "string" ? value : undefined;
  return (
    <Select value={current} onValueChange={(v) => onChange(v)}>
      <SelectTrigger>
        <SelectValue placeholder="Select…" />
      </SelectTrigger>
      <SelectContent>
        {values.map((v) => (
          <SelectItem key={v} value={v}>
            {labelFor(v)}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
}
