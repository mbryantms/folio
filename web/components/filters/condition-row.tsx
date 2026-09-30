"use client";

import { Trash2 } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Combobox } from "@/components/ui/combobox";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import type { Condition, Field, Op } from "@/lib/api/types";

import {
  OP_LABELS,
  fieldLabel,
  fieldsFor,
  opTakesNoValue,
  specFor,
  type FieldSpec,
  type ViewEntity,
} from "./field-registry";
import { TextEditor } from "./value-editors/TextEditor";
import { NumberEditor } from "./value-editors/NumberEditor";
import { DateEditor } from "./value-editors/DateEditor";
import { EnumEditor } from "./value-editors/EnumEditor";
import { MultiSelectEditor } from "./value-editors/MultiSelectEditor";

export type ConditionRowProps = {
  condition: Condition;
  /** Optional library scope passed to async option lookups. */
  library?: string;
  /** Which root the view compiles against — narrows the field picker
   *  to fields available there (WP-5.4). Defaults to series. */
  entity?: ViewEntity;
  onChange: (next: Condition) => void;
  onRemove: () => void;
};

const FIELD_OPTIONS: Record<ViewEntity, { value: Field; label: string }[]> = {
  series: fieldsFor("series").map((s) => ({
    value: s.id,
    label: fieldLabel(s, "series"),
  })),
  issue: fieldsFor("issue").map((s) => ({
    value: s.id,
    label: fieldLabel(s, "issue"),
  })),
};

export function ConditionRow({
  condition,
  library,
  entity = "series",
  onChange,
  onRemove,
}: ConditionRowProps) {
  const spec = specFor(condition.field);
  const valueEditor = renderValueEditor(spec, condition, library, (v) =>
    onChange({ ...condition, value: v }),
  );

  return (
    <div className="grid grid-cols-1 items-start gap-2 sm:grid-cols-[minmax(160px,1fr)_minmax(140px,1fr)_2fr_auto]">
      <Combobox
        options={FIELD_OPTIONS[entity]}
        value={condition.field}
        onChange={(next) => {
          const nextSpec = specFor(next as Field);
          const nextOp = nextSpec.allowedOps.includes(condition.op)
            ? condition.op
            : nextSpec.allowedOps[0];
          onChange({
            ...condition,
            field: next as Field,
            op: nextOp,
            value: undefined,
          });
        }}
        placeholder="Choose a field…"
        searchPlaceholder="Search fields…"
      />
      <Select
        value={condition.op}
        onValueChange={(next) => {
          const op = next as Op;
          // Value-less ops (is_empty, …) drop any stale value.
          onChange(
            opTakesNoValue(op)
              ? { ...condition, op, value: undefined }
              : { ...condition, op },
          );
        }}
      >
        <SelectTrigger>
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          {spec.allowedOps.map((op) => (
            <SelectItem key={op} value={op}>
              {OP_LABELS[op]}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
      <div>{valueEditor}</div>
      <Button
        type="button"
        variant="ghost"
        size="icon"
        onClick={onRemove}
        aria-label="Remove condition"
      >
        <Trash2 className="h-4 w-4" />
      </Button>
    </div>
  );
}

function renderValueEditor(
  spec: FieldSpec,
  condition: Condition,
  library: string | undefined,
  onValueChange: (value: unknown) => void,
) {
  if (opTakesNoValue(condition.op)) return null;

  switch (spec.kind) {
    case "text":
      return (
        <TextEditor
          value={condition.value}
          onChange={(v) => onValueChange(v)}
        />
      );
    case "number":
      return (
        <NumberEditor
          op={condition.op}
          value={condition.value}
          onChange={(v) => onValueChange(v)}
        />
      );
    case "date":
      return (
        <DateEditor
          op={condition.op}
          value={condition.value}
          onChange={(v) => onValueChange(v)}
        />
      );
    case "enum":
      return (
        <EnumEditor
          op={condition.op}
          value={condition.value}
          values={spec.enumValues ?? []}
          labels={spec.enumLabels}
          onChange={(v) => onValueChange(v)}
        />
      );
    case "multi":
      return (
        <MultiSelectEditor
          value={condition.value}
          onChange={(v) => onValueChange(v)}
          endpoint={spec.optionsEndpoint}
          library={library}
        />
      );
    case "uuid":
      // Library is the only UUID field today. Use MultiSelectEditor for
      // `in/not_in`; a TextEditor placeholder for direct equals/not.
      if (condition.op === "in" || condition.op === "not_in") {
        return (
          <MultiSelectEditor
            value={condition.value}
            onChange={(v) => onValueChange(v)}
            endpoint={spec.optionsEndpoint}
            library={library}
          />
        );
      }
      return (
        <TextEditor
          value={condition.value}
          onChange={(v) => onValueChange(v)}
          placeholder="Library ID"
        />
      );
  }
}
