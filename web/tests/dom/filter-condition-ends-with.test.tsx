// @vitest-environment jsdom
/**
 * UX-8: the filter builder offers `ends_with` ("ends with") on text
 * fields, next to `starts_with`. Picking it keeps the typed value (it is a
 * value-taking op) and the text editor stays mounted; numeric fields
 * don't offer it.
 */
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { ConditionRow } from "@/components/filters/condition-row";
import { OP_LABELS, specFor } from "@/components/filters/field-registry";
import type { Condition } from "@/lib/api/types";

function opTrigger() {
  // Field picker is the first combobox, the op Select the second.
  return screen.getAllByRole("combobox")[1]!;
}

describe("filter builder ends_with op", () => {
  it("is offered on text fields only, labelled 'ends with'", () => {
    expect(OP_LABELS.ends_with).toBe("ends with");
    for (const f of ["name", "publisher", "title", "summary"] as const) {
      expect(specFor(f).allowedOps).toContain("ends_with");
    }
    expect(specFor("year").allowedOps).not.toContain("ends_with");
    expect(specFor("genres").allowedOps).not.toContain("ends_with");
  });

  it("picking 'ends with' keeps the text value", () => {
    const onChange = vi.fn();
    const condition: Condition = {
      group_id: 0,
      field: "name",
      op: "contains",
      value: "Annual",
    };
    render(
      <ConditionRow
        condition={condition}
        onChange={onChange}
        onRemove={() => {}}
      />,
    );
    fireEvent.keyDown(opTrigger(), { key: "Enter" });
    const option = screen.getByRole("option", { name: "ends with" });
    fireEvent.keyDown(option, { key: "Enter" });
    expect(onChange).toHaveBeenCalledWith({
      ...condition,
      op: "ends_with",
      value: "Annual",
    });
  });

  it("renders the current ends_with op and its text editor", () => {
    render(
      <ConditionRow
        condition={{
          group_id: 0,
          field: "name",
          op: "ends_with",
          value: "Annual",
        }}
        onChange={() => {}}
        onRemove={() => {}}
      />,
    );
    expect(opTrigger().textContent).toContain("ends with");
    expect(screen.getByDisplayValue("Annual")).toBeTruthy();
  });
});
