// @vitest-environment jsdom
/**
 * Regression guards for the Folio wrappers the axe route sweep
 * (`tests/e2e/a11y.spec.ts`, audit N-A9) fixed:
 *
 *   - `FilterPill` emitted `aria-pressed` on every pill, including the
 *     bookmarks kind filter rendered as `role="tab"` (axe
 *     `aria-allowed-attr`: `aria-pressed` is only valid on a button).
 *   - the vendored `ui/slider` renders unnamed thumbs; the Folio
 *     `folio/slider` names each one (axe `aria-input-field-name`).
 *   - `Combobox` has `role="combobox"` on its trigger, which takes no
 *     name from content; the new `aria-label` prop reaches it (axe
 *     `button-name` on the bookmarks series filter).
 */
import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { Combobox } from "@/components/folio/combobox";
import { FilterPill } from "@/components/folio/filter-pill";
import { Slider } from "@/components/folio/slider";

describe("FilterPill state attribute follows its role", () => {
  it("is a pressed toggle button by default", () => {
    render(<FilterPill active>Notes</FilterPill>);
    const pill = screen.getByRole("button", { name: "Notes" });
    expect(pill.getAttribute("aria-pressed")).toBe("true");
  });

  it("drops aria-pressed when rendered as a tab", () => {
    render(
      <div role="tablist">
        <FilterPill role="tab" active aria-selected>
          All
        </FilterPill>
      </div>,
    );
    const tab = screen.getByRole("tab", { name: "All" });
    expect(tab.hasAttribute("aria-pressed")).toBe(false);
    expect(tab.getAttribute("aria-selected")).toBe("true");
  });

  it("drops aria-pressed when rendered as a radio", () => {
    render(
      <div role="radiogroup">
        <FilterPill role="radio" active aria-checked>
          Grid
        </FilterPill>
      </div>,
    );
    const radio = screen.getByRole("radio", { name: "Grid" });
    expect(radio.hasAttribute("aria-pressed")).toBe(false);
  });
});

describe("Slider wrapper names its thumbs", () => {
  it("labels a single thumb", () => {
    render(<Slider value={[30]} min={0} max={100} thumbLabel="Card size" />);
    expect(screen.getByRole("slider", { name: "Card size" })).toBeTruthy();
  });

  it("labels each thumb of a range", () => {
    render(
      <Slider
        value={[1, 4]}
        min={0}
        max={5}
        thumbLabel={["Minimum rating", "Maximum rating"]}
      />,
    );
    expect(screen.getByRole("slider", { name: "Minimum rating" })).toBeTruthy();
    expect(screen.getByRole("slider", { name: "Maximum rating" })).toBeTruthy();
  });
});

describe("Combobox trigger name", () => {
  it("takes the aria-label prop", () => {
    render(
      <Combobox
        options={[{ value: "a", label: "Alpha" }]}
        onChange={() => {}}
        placeholder="Series"
        aria-label="Filter by series"
      />,
    );
    expect(
      screen.getByRole("combobox", { name: "Filter by series" }),
    ).toBeTruthy();
  });
});
