"use client";

import { useRouter, useSearchParams } from "next/navigation";

import { FIELD_SPECS, specFor } from "@/components/filters/field-registry";
import type { Condition, Field, Op } from "@/lib/api/types";

import { NewFilterViewDialog } from "./AddViewButton";

/** Reads `?quick_field=...&quick_value=...` query params on
 *  /settings/views and opens the New filter view dialog pre-filled
 *  with that one condition. Closing the dialog drops the params via
 *  `router.replace`, which unmounts this component on the next render
 *  — so dialog open-state lives entirely in the URL, no `useState`
 *  needed. Used by the M9 quick-apply flow from chip lists. */
export function QuickApplyPrefill() {
  const router = useRouter();
  const sp = useSearchParams();
  const fieldParam = sp.get("quick_field");
  const valueParam = sp.get("quick_value");

  if (!fieldParam || !valueParam) return null;
  const valid = FIELD_SPECS.some((s) => s.id === fieldParam);
  if (!valid) return null;

  const field = fieldParam as Field;
  const spec = specFor(field);
  const op: Op = spec.kind === "multi" ? "includes_any" : "equals";
  const value = spec.kind === "multi" ? [valueParam] : valueParam;
  const condition: Condition = { group_id: 0, field, op, value };

  function handleOpenChange(next: boolean) {
    if (next) return;
    const remaining = new URLSearchParams(sp.toString());
    remaining.delete("quick_field");
    remaining.delete("quick_value");
    const qs = remaining.toString();
    router.replace(qs ? `/settings/views?${qs}` : "/settings/views");
  }

  return (
    <NewFilterViewDialog
      open
      onOpenChange={handleOpenChange}
      autoPin
      initial={{
        name: `${valueParam} ${spec.label.toLowerCase()}`,
        matchMode: "all",
        conditions: [condition],
      }}
    />
  );
}
