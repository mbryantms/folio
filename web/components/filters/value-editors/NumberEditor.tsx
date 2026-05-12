"use client";

import type { Op } from "@/lib/api/types";
import { Input } from "@/components/ui/input";

export type NumberEditorProps = {
  op: Op;
  value: unknown;
  onChange: (value: number | [number, number]) => void;
};

/** `between` takes a `[lower, upper]` tuple; everything else is a
 *  scalar. The compiler validates the shape — bad input shows up as a
 *  422 with a clear message. */
export function NumberEditor({ op, value, onChange }: NumberEditorProps) {
  if (op === "between") {
    const tuple = Array.isArray(value) ? (value as [unknown, unknown]) : [];
    const lo = typeof tuple[0] === "number" ? String(tuple[0]) : "";
    const hi = typeof tuple[1] === "number" ? String(tuple[1]) : "";
    return (
      <div className="flex items-center gap-2">
        <Input
          type="number"
          value={lo}
          onChange={(e) => {
            const next = parseFloat(e.target.value);
            const upper = typeof tuple[1] === "number" ? tuple[1] : 0;
            onChange([Number.isFinite(next) ? next : 0, upper]);
          }}
          placeholder="From"
          className="w-24"
        />
        <span className="text-muted-foreground text-xs">to</span>
        <Input
          type="number"
          value={hi}
          onChange={(e) => {
            const next = parseFloat(e.target.value);
            const lower = typeof tuple[0] === "number" ? tuple[0] : 0;
            onChange([lower, Number.isFinite(next) ? next : 0]);
          }}
          placeholder="To"
          className="w-24"
        />
      </div>
    );
  }
  const v = typeof value === "number" ? String(value) : "";
  return (
    <Input
      type="number"
      value={v}
      onChange={(e) => {
        const next = parseFloat(e.target.value);
        if (Number.isFinite(next)) onChange(next);
      }}
      placeholder="Value"
    />
  );
}
