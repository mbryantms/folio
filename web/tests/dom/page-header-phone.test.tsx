// @vitest-environment jsdom
/**
 * `<PageHeader>` phone tightening: the desktop classes are untouched;
 * below `sm` the header shrinks, the description clamps to two lines
 * and `phoneDescription="hidden"` drops it entirely. `actionsClassName`
 * lets a page hide its desktop toolbar when a `PhoneControlsBar`
 * replaces it.
 */
import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { PageHeader } from "@/components/admin/PageHeader";

describe("PageHeader phone classes", () => {
  it("keeps the desktop classes and adds the max-sm: hooks", () => {
    render(<PageHeader title="Bookmarks" description="Every marker." />);
    const header = screen.getByRole("banner");
    for (const c of ["mb-6", "pb-4", "max-sm:mb-4", "max-sm:pb-3"]) {
      expect(header.className).toContain(c);
    }
    const h1 = screen.getByRole("heading", { level: 1 });
    expect(h1.className).toContain("text-2xl");
    expect(h1.className).toContain("max-sm:text-xl");
    const p = screen.getByText("Every marker.");
    expect(p.className).toContain("text-sm");
    expect(p.className).toContain("max-sm:text-xs");
    expect(p.className).toContain("max-sm:line-clamp-2");
    expect(p.className).not.toContain("max-sm:hidden");
  });

  it('phoneDescription="hidden" hides the description below sm only', () => {
    render(
      <PageHeader
        title="Browse"
        description="Flavour text"
        phoneDescription="hidden"
      />,
    );
    const p = screen.getByText("Flavour text");
    expect(p.className).toContain("max-sm:hidden");
    expect(p.className).toContain("text-sm");
  });

  it("passes actionsClassName through to the actions wrapper", () => {
    render(
      <PageHeader
        title="X"
        actions={<button type="button">Act</button>}
        actionsClassName="max-sm:hidden"
      />,
    );
    const wrapper = screen.getByRole("button", { name: "Act" }).parentElement;
    expect(wrapper?.className).toContain("flex flex-wrap items-center gap-2");
    expect(wrapper?.className).toContain("max-sm:hidden");
  });
});
