/** Single-field PATCH body builder for the inline metadata editor (B12). */
import { describe, expect, it } from "vitest";

import { buildFieldPatch } from "@/components/library/InlineIssueFieldEdit";

describe("buildFieldPatch", () => {
  it("sends the trimmed string for a text field", () => {
    expect(buildFieldPatch("publisher", "  Marvel  ", "text")).toEqual({
      publisher: "Marvel",
    });
  });

  it("clears the field (null) when the value is emptied", () => {
    expect(buildFieldPatch("publisher", "", "text")).toEqual({
      publisher: null,
    });
    expect(buildFieldPatch("story_arc", "   ", "text")).toEqual({
      story_arc: null,
    });
  });

  it("parses a number field to an int", () => {
    expect(buildFieldPatch("volume", "3", "number")).toEqual({ volume: 3 });
    // Leading/trailing space is tolerated.
    expect(buildFieldPatch("volume", " 12 ", "number")).toEqual({ volume: 12 });
  });

  it("clears a number field when emptied or non-numeric", () => {
    expect(buildFieldPatch("volume", "", "number")).toEqual({ volume: null });
    expect(buildFieldPatch("volume", "abc", "number")).toEqual({
      volume: null,
    });
  });

  it("passes an enum value straight through", () => {
    expect(buildFieldPatch("age_rating", "Teen", "enum")).toEqual({
      age_rating: "Teen",
    });
  });
});
