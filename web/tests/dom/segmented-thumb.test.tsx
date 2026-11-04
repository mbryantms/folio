// @vitest-environment jsdom
/**
 * Segmented controls slide one thumb behind the selected option
 * (folio-flair pre-merge polish, 2026-11-04); the buttons keep their
 * `aria-pressed` semantics and the selected one stops painting its own
 * fill once the thumb is measured.
 */
import { fireEvent, render, screen } from "@testing-library/react";
import { useState } from "react";
import { describe, expect, it } from "vitest";

import { SegmentedControl } from "@/components/settings/SegmentedControl";

function Harness() {
  const [v, setV] = useState<"a" | "b" | "c">("a");
  return (
    <SegmentedControl
      value={v}
      onChange={setV}
      ariaLabel="Pick"
      options={[
        { value: "a", label: "A" },
        { value: "b", label: "B" },
        { value: "c", label: "C" },
      ]}
    />
  );
}

describe("SegmentedControl thumb", () => {
  it("renders one thumb and keeps aria-pressed", () => {
    render(<Harness />);
    const group = screen.getByRole("group", { name: "Pick" });
    expect(group.dataset.thumb).toBe("");
    expect(
      group.querySelectorAll('[data-slot="segmented-thumb"]'),
    ).toHaveLength(1);
    const b = screen.getByRole("button", { name: "B" });
    fireEvent.click(b);
    expect(b.getAttribute("aria-pressed")).toBe("true");
    expect(
      screen.getByRole("button", { name: "A" }).getAttribute("aria-pressed"),
    ).toBe("false");
    expect(b.className).toContain("group-data-[thumb]/seg:bg-transparent");
  });
});
