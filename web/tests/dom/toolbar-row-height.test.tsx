// @vitest-environment jsdom
/**
 * Toolbar-row convention: every inline control in a page-header toolbar
 * is `h-9` (36px) on fine pointers so the row reads as one band next to
 * the `<Input>` (also `h-9`). `Button size="sm"` is 32px, which is how
 * /bookmarks shipped with Select + Export 4px shorter than the search
 * box and the card-size trigger beside them, and /log with Add widget /
 * Export CSV / Reset shorter than the 38px range selector. The shared
 * controls below bake the height in so no call site has to remember it.
 */
import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

vi.mock("@/lib/api/mutations", () => ({
  useAddLogWidget: () => ({ mutate: vi.fn(), isPending: false }),
}));

import { ActivityRangeSelector } from "@/components/activity/ActivityRangeSelector";
import { CardSizeOptions } from "@/components/library/CardSizeOptions";
import { SelectModeButton } from "@/components/library/SelectModeButton";
import { AddWidgetMenu } from "@/components/log/AddWidgetMenu";
import { NotesExportMenu } from "@/components/markers/NotesExportMenu";
import { Input } from "@/components/ui/input";

const H9 = /\bh-9\b/;
/** `size="sm"` adds `text-xs`; a toolbar control must keep the base
 *  `text-sm` so its label matches the `<Input>` beside it. */
const XS = /\btext-xs\b/;

describe("toolbar row height", () => {
  it("shared header controls all carry h-9 like the Input beside them", () => {
    render(
      <>
        <Input aria-label="Search" />
        <SelectModeButton active={false} onEnter={() => {}} onExit={() => {}} />
        <NotesExportMenu />
        <CardSizeOptions
          cardSize={160}
          onCardSize={() => {}}
          min={100}
          max={300}
          step={10}
          defaultSize={160}
        />
      </>,
    );
    expect(screen.getByLabelText("Search").className).toMatch(H9);
    expect(screen.getByRole("button", { name: "Select" }).className).toMatch(
      H9,
    );
    expect(
      screen.getByRole("button", { name: "Export notes" }).className,
    ).toMatch(H9);
    expect(
      screen.getByRole("button", { name: "View options" }).className,
    ).toMatch(H9);
    for (const name of ["Select", "Export notes", "View options"]) {
      const cls = screen.getByRole("button", { name }).className;
      expect(cls).toMatch(/\btext-sm\b/);
      expect(cls).not.toMatch(XS);
    }
  });

  it("keeps h-9 when a call site passes its own classes", () => {
    render(
      <SelectModeButton
        active
        onEnter={() => {}}
        onExit={() => {}}
        className="ml-auto"
      />,
    );
    const btn = screen.getByRole("button", { name: "Cancel" });
    expect(btn.className).toMatch(H9);
    expect(btn.className).toMatch(/\bml-auto\b/);
  });

  it("the /log header controls match: range selector + Add widget", () => {
    render(
      <>
        <ActivityRangeSelector value="30d" onChange={() => {}} />
        <AddWidgetMenu current={[]} />
      </>,
    );
    const range = screen.getByRole("group", { name: "Stats range" });
    expect(range.className).toMatch(H9);
    // Joins the 44px touch floor with its Button siblings (rule 9).
    expect(range.classList.contains("folio-control")).toBe(true);
    // The pills stretch to the group instead of adding their own
    // vertical padding on top of the 36px box.
    for (const b of range.querySelectorAll("button")) {
      expect(b.className).toMatch(/\bpy-1\.5\b/);
    }
    expect(range.className).toMatch(/\[&>button\]:py-0/);
    expect(
      screen.getByRole("button", { name: "Add widget" }).className,
    ).toMatch(H9);
  });
});
