import { describe, expect, it } from "vitest";

import {
  FIELD_SPECS,
  OP_LABELS,
  specFor,
} from "@/components/filters/field-registry";
import type { Field, Op } from "@/lib/api/types";

const ALL_FIELDS: Field[] = [
  "library",
  "name",
  "year",
  "volume",
  "total_issues",
  "publisher",
  "imprint",
  "status",
  "age_rating",
  "language_code",
  "created_at",
  "updated_at",
  "genres",
  "tags",
  "writer",
  "penciller",
  "inker",
  "colorist",
  "letterer",
  "cover_artist",
  "editor",
  "translator",
  "read_progress",
  "last_read",
  "read_count",
];

const ALL_OPS: Op[] = [
  "contains",
  "starts_with",
  "equals",
  "not_equals",
  "is",
  "is_not",
  "in",
  "not_in",
  "gt",
  "gte",
  "lt",
  "lte",
  "between",
  "before",
  "after",
  "relative",
  "includes_any",
  "includes_all",
  "excludes",
  "is_true",
  "is_false",
];

describe("filter field registry", () => {
  it("covers every Field variant from the API", () => {
    const ids = FIELD_SPECS.map((s) => s.id).sort();
    expect(ids).toEqual([...ALL_FIELDS].sort());
  });

  it("specFor returns the registry entry for each field", () => {
    for (const f of ALL_FIELDS) {
      const spec = specFor(f);
      expect(spec.id).toBe(f);
      expect(spec.label.length).toBeGreaterThan(0);
      expect(spec.allowedOps.length).toBeGreaterThan(0);
    }
  });

  it("each enum field has a non-empty enumValues list", () => {
    for (const spec of FIELD_SPECS) {
      if (spec.kind === "enum") {
        expect(spec.enumValues?.length ?? 0).toBeGreaterThan(0);
      }
    }
  });

  it("OP_LABELS covers every Op", () => {
    for (const op of ALL_OPS) {
      expect(OP_LABELS[op]).toBeTruthy();
    }
  });

  it("multi fields all wire an optionsEndpoint", () => {
    const multi = FIELD_SPECS.filter((s) => s.kind === "multi");
    for (const spec of multi) {
      expect(spec.optionsEndpoint).toBeDefined();
    }
  });
});
