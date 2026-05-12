"use client";

import type { Op } from "@/lib/api/types";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";

export type DateEditorProps = {
  op: Op;
  value: unknown;
  onChange: (
    value: string | [string, string] | { count: number; unit: string },
  ) => void;
};

const RELATIVE_UNITS = [
  { value: "days", label: "Days" },
  { value: "weeks", label: "Weeks" },
  { value: "months", label: "Months" },
  { value: "years", label: "Years" },
];

export function DateEditor({ op, value, onChange }: DateEditorProps) {
  if (op === "between") {
    const tuple = Array.isArray(value) ? (value as [unknown, unknown]) : [];
    const from = typeof tuple[0] === "string" ? tuple[0] : "";
    const to = typeof tuple[1] === "string" ? tuple[1] : "";
    return (
      <div className="flex items-center gap-2">
        <Input
          type="date"
          value={from}
          onChange={(e) => onChange([e.target.value, to])}
          className="w-40"
        />
        <span className="text-muted-foreground text-xs">to</span>
        <Input
          type="date"
          value={to}
          onChange={(e) => onChange([from, e.target.value])}
          className="w-40"
        />
      </div>
    );
  }
  if (op === "relative") {
    const obj =
      value && typeof value === "object"
        ? (value as Record<string, unknown>)
        : {};
    const count =
      typeof obj.count === "number" && Number.isFinite(obj.count)
        ? obj.count
        : 7;
    const unit = typeof obj.unit === "string" ? obj.unit : "days";
    return (
      <div className="flex items-center gap-2">
        <Input
          type="number"
          min={1}
          value={count}
          onChange={(e) => {
            const next = parseInt(e.target.value, 10);
            onChange({ count: Number.isFinite(next) ? next : 1, unit });
          }}
          className="w-20"
        />
        <Select
          value={unit}
          onValueChange={(u) => onChange({ count, unit: u })}
        >
          <SelectTrigger className="w-32">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {RELATIVE_UNITS.map((u) => (
              <SelectItem key={u.value} value={u.value}>
                {u.label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>
    );
  }
  const v = typeof value === "string" ? value : "";
  return (
    <Input
      type="date"
      value={v}
      onChange={(e) => onChange(e.target.value)}
      className="w-40"
    />
  );
}
