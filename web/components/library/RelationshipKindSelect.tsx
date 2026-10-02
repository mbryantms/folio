"use client";

/**
 * `<RelationshipKindSelect>` — WP-7.5 grouped "Relationship" picker
 * (Story · Publication history · Editions & contents · Advanced), fed by
 * the server's kind catalogue (`GET /relationship-kinds`). Used by the
 * series page's add form and the admin review page's "Edit kind".
 */

import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectLabel,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { useRelationshipKinds } from "@/lib/api/queries";
import type { RelationshipKind } from "@/lib/api/types";
import { groupedKinds } from "@/lib/relationships";

export function RelationshipKindSelect({
  id,
  value,
  onChange,
  ariaLabel = "Relationship",
}: {
  id?: string;
  value: RelationshipKind;
  onChange: (kind: RelationshipKind) => void;
  ariaLabel?: string;
}) {
  const catalogue = useRelationshipKinds();
  const groups = groupedKinds(catalogue.data);
  return (
    <Select
      value={value}
      onValueChange={(v) => onChange(v as RelationshipKind)}
      disabled={groups.length === 0}
    >
      <SelectTrigger id={id} aria-label={ariaLabel}>
        <SelectValue placeholder="Loading…" />
      </SelectTrigger>
      <SelectContent className="max-h-80">
        {groups.map((g) => (
          <SelectGroup key={g.group}>
            <SelectLabel>{g.label}</SelectLabel>
            {g.kinds.map((k) => (
              <SelectItem key={k.kind} value={k.kind}>
                {k.label}
              </SelectItem>
            ))}
          </SelectGroup>
        ))}
      </SelectContent>
    </Select>
  );
}
