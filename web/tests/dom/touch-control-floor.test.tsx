// @vitest-environment jsdom
/**
 * Touch target (`docs/dev/phone-layout.md`, rule 9). Under
 * `(pointer: coarse)` `globals.css` gives `.folio-button`, `.folio-control`
 * and `.folio-tap` an invisible 44px-tall tap area via `::after` — the
 * visible control keeps shadcn's size, so an iPad toolbar is the same
 * 36px band as the tab strip beside it. jsdom cannot evaluate the media
 * query, so this guards the halves separately: the CSS rule is the hit
 * area and not a size floor, and the controls that are not `<Button>`
 * carry the classes.
 */
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { CblStatsPillsStatic } from "@/components/cbl/CblStatsPills";
import { ModeSegmentedToggle } from "@/components/library/LibraryGridToolbar";
import { FilterPill } from "@/components/folio/filter-pill";
import { NativeSelect } from "@/components/folio/native-select";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Select, SelectTrigger, SelectValue } from "@/components/folio/select";

describe("touch-target floor", () => {
  it("grows a 44px tap area under pointer: coarse instead of the box", () => {
    const css = readFileSync(join(process.cwd(), "styles/globals.css"), "utf8");
    const at = css.indexOf("@media (pointer: coarse)");
    expect(at).toBeGreaterThan(0);
    const block = css.slice(at, css.indexOf("\n}\n", at));
    expect(block).toMatch(
      /\.folio-button::after,\s*\.folio-control::after,\s*\.folio-tap::after\s*\{[^}]*height:\s*max\(100%,\s*44px\);/,
    );
    // The visible size is shadcn's on every pointer: no floor.
    expect(block).not.toMatch(/min-height:\s*44px/);
    expect(block).not.toMatch(/min-width:\s*44px/);
    // A control that contains controls (segmented group) gets no tap
    // area of its own: it would paint above the children and take their
    // taps (iOS, the Series / Issues toggle, 2026-10-24).
    expect(block).toMatch(
      /\.folio-control:has\([\s\S]*?button[\s\S]*?\)::after\s*\{\s*content:\s*none;/,
    );
  });

  it("toolbar controls carry the tap-area hook: data-slot or folio-control", () => {
    render(
      <>
        <Button>Go</Button>
        <Input aria-label="Search" />
        <Select>
          <SelectTrigger aria-label="Sort">
            <SelectValue placeholder="Sort" />
          </SelectTrigger>
        </Select>
        <ModeSegmentedToggle mode="series" onMode={() => {}} />
      </>,
    );
    expect(
      screen.getByRole("button", { name: "Go" }).dataset.slot === "button",
    ).toBe(true);
    // Pristine shadcn files carry no Folio class; the CSS hooks their
    // `data-slot` instead.
    expect(screen.getByLabelText("Search").dataset.slot === "input").toBe(true);
    expect(
      screen.getByRole("combobox", { name: "Sort" }).dataset.slot ===
        "select-trigger",
    ).toBe(true);
    const toggle = screen.getByRole("group", {
      name: "Browse series or issues",
    });
    expect(toggle.classList.contains("folio-control")).toBe(true);
    // The segments stretch to the group's height instead of pinning h-8,
    // so a 44px group on touch does not leave two 32px pills inside it.
    for (const b of toggle.querySelectorAll("button")) {
      expect(b.className).not.toMatch(/\bh-8\b/);
    }
  });

  it("the sort select, CBL pills, tabs and pills carry the tap-area classes", () => {
    const { container } = render(
      <>
        <NativeSelect
          aria-label="Sort"
          options={[{ value: "newest", label: "Newest" }]}
        />
        <CblStatsPillsStatic
          size="header"
          stats={{
            matched: 30,
            total: 860,
            read_count: 12,
            ambiguous: 0,
            manual: 0,
            missing: 830,
          }}
        />
        <Tabs defaultValue="credits">
          <TabsList>
            <TabsTrigger value="credits">Credits</TabsTrigger>
          </TabsList>
        </Tabs>
        <FilterPill active={false}>Notes</FilterPill>
      </>,
    );
    expect(
      screen
        .getByRole("combobox", { name: "Sort" })
        .classList.contains("folio-control"),
    ).toBe(true);
    // Both pills carry the floor; on an iPad they sat 36px beside 44px
    // buttons in the CBL view header.
    // Select + the two pills carry folio-control; none grows its box.
    const controls = container.querySelectorAll(".folio-control");
    expect(controls.length).toBeGreaterThanOrEqual(3);
    for (const el of controls) {
      expect(el.className).not.toMatch(/pointer-coarse:h-/);
    }
    // Tab triggers and filter pills get the tap area too.
    expect(
      screen
        .getByRole("tab", { name: "Credits" })
        .classList.contains("folio-tap"),
    ).toBe(true);
    // The strip around them scrolls horizontally only: with
    // `overflow-x-auto` alone the tap areas poking past the 36px strip
    // gave it a vertical scroll range, and iOS let a finger drag the tab
    // labels up and down inside the bar (2026-10-24).
    expect(screen.getByRole("tablist").className).toMatch(
      /\boverflow-y-hidden\b/,
    );
    expect(
      screen
        .getByRole("button", { name: /Notes/ })
        .classList.contains("folio-tap"),
    ).toBe(true);
  });
});
