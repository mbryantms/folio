import { describe, expect, it } from "vitest";

import {
  FIELD_SPECS,
  OP_LABELS,
  entityForKind,
  fieldLabel,
  fieldsFor,
  kindForEntity,
  opTakesNoValue,
  specFor,
} from "@/components/filters/field-registry";
import { switchEntity } from "@/components/filters/filter-builder";
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
  "characters",
  "teams",
  "locations",
  "read_progress",
  "last_read",
  "read_count",
  "read_status",
  "unread_issues",
  "collection_completeness",
  "metadata_completeness",
  "special_type",
  "format",
  "story_arc",
  "title",
  "rating",
  "has_notes",
  "has_bookmarks",
  "has_highlights",
];

const ALL_OPS: Op[] = [
  "contains",
  "not_contains",
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
  "is_empty",
  "is_not_empty",
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

  // WP-5.4 — per-entity availability mirrors `source` / `issue_source`
  // in crates/server/src/views/registry.rs.
  it("issue-only fields are hidden from series views", () => {
    const series = fieldsFor("series").map((s) => s.id);
    const issue = fieldsFor("issue").map((s) => s.id);
    for (const f of [
      "special_type",
      "format",
      "story_arc",
      "title",
    ] as Field[]) {
      expect(series).not.toContain(f);
      expect(issue).toContain(f);
    }
  });

  it("series rollups are hidden from issue views", () => {
    const issue = fieldsFor("issue").map((s) => s.id);
    for (const f of [
      "read_progress",
      "last_read",
      "read_count",
      "unread_issues",
      "collection_completeness",
      "metadata_completeness",
    ] as Field[]) {
      expect(issue).not.toContain(f);
    }
    expect(issue).toContain("read_status");
    expect(issue).toContain("rating");
  });

  it("is_empty is offered on nullable fields but not on computed ones", () => {
    expect(specFor("story_arc").allowedOps).toContain("is_empty");
    expect(specFor("genres").allowedOps).toContain("is_not_empty");
    expect(specFor("read_status").allowedOps).not.toContain("is_empty");
    expect(specFor("name").allowedOps).not.toContain("is_empty");
    expect(opTakesNoValue("is_empty")).toBe(true);
    expect(opTakesNoValue("equals")).toBe(false);
  });

  it("marker filters are boolean and offered on both entities (WP-5.7)", () => {
    for (const f of [
      "has_notes",
      "has_bookmarks",
      "has_highlights",
    ] as Field[]) {
      const spec = specFor(f);
      expect(spec.kind).toBe("bool");
      expect(spec.allowedOps).toEqual(["is_true", "is_false"]);
      expect(fieldsFor("series").map((s) => s.id)).toContain(f);
      expect(fieldsFor("issue").map((s) => s.id)).toContain(f);
    }
  });

  it("kind <-> entity round-trips and issue labels override", () => {
    expect(entityForKind("filter_issues")).toBe("issue");
    expect(entityForKind("filter_series")).toBe("series");
    expect(kindForEntity("issue")).toBe("filter_issues");
    expect(fieldLabel(specFor("name"), "issue")).toBe("Series Name");
    expect(fieldLabel(specFor("name"), "series")).toBe("Name");
  });

  it("switching the builder to issues drops series-only conditions + sorts", () => {
    const next = switchEntity(
      {
        name: "x",
        description: "",
        entity: "series",
        matchMode: "all",
        conditions: [
          { field: "unread_issues", op: "gt", value: 1 },
          { field: "year", op: "equals", value: 2019 },
        ],
        sortField: "read_progress",
        sortOrder: "desc",
        resultLimit: 12,
      },
      "issue",
    );
    expect(next.entity).toBe("issue");
    expect(next.conditions.map((c) => c.field)).toEqual(["year"]);
    expect(next.sortField).toBe("created_at");
  });
});
