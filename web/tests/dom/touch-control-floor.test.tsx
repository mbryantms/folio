// @vitest-environment jsdom
/**
 * Touch-target floor (`docs/dev/phone-layout.md`, rule 9). Under
 * `(pointer: coarse)` `globals.css` floors `.folio-button` *and*
 * `.folio-control` at 44px. jsdom cannot evaluate the media query, so
 * this guards the two halves separately: the CSS rule names both
 * classes with the same floor, and the inline toolbar controls that are
 * not `<Button>` carry `folio-control`. Without the class an iPad
 * toolbar showed 44px buttons beside a 36px search box and sort select.
 */
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { CblStatsPillsStatic } from "@/components/cbl/CblStatsPills";
import { ModeSegmentedToggle } from "@/components/library/LibraryGridToolbar";
import { NativeSelect } from "@/components/ui/native-select";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Select, SelectTrigger, SelectValue } from "@/components/ui/select";

describe("touch-target floor", () => {
  it("floors .folio-button and .folio-control together under pointer: coarse", () => {
    const css = readFileSync(join(process.cwd(), "styles/globals.css"), "utf8");
    const at = css.indexOf("@media (pointer: coarse)");
    expect(at).toBeGreaterThan(0);
    const block = css.slice(at, css.indexOf("\n}\n", at));
    expect(block).toMatch(
      /\.folio-button,\s*\.folio-control\s*\{\s*min-height:\s*44px;/,
    );
  });

  it("marks the non-Button toolbar controls with folio-control", () => {
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
      screen
        .getByRole("button", { name: "Go" })
        .classList.contains("folio-button"),
    ).toBe(true);
    expect(
      screen.getByLabelText("Search").classList.contains("folio-control"),
    ).toBe(true);
    expect(
      screen
        .getByRole("combobox", { name: "Sort" })
        .classList.contains("folio-control"),
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

  it("floors the /bookmarks sort select and the CBL header pills too", () => {
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
      </>,
    );
    expect(
      screen
        .getByRole("combobox", { name: "Sort" })
        .classList.contains("folio-control"),
    ).toBe(true);
    // Both pills carry the floor; on an iPad they sat 36px beside 44px
    // buttons in the CBL view header.
    // Select + the two pills.
    expect(
      container.querySelectorAll(".folio-control").length,
    ).toBeGreaterThanOrEqual(3);
  });
});
