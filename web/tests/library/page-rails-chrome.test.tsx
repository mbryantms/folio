// @vitest-environment jsdom
/**
 * `<PageRails>` chrome (phone-layout rule 3): a rails page pins its two
 * controls at every width. From `sm` up the heading stays in flow and a
 * `StickyToolbarRow` beneath it carries card size + page actions; below
 * `sm` the title rides in the sticky `PhoneControlsBar` beside View
 * options and `⋯`, with the description following in flow. jsdom has no
 * CSS, so both compositions render — the assertions pin the wiring.
 */
import { render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

vi.mock("@/lib/api/queries", () => ({
  useSavedViews: () => ({ data: { items: [] }, isLoading: false }),
  useLibraryList: () => ({ data: { items: [] } }),
  useMe: () => ({ data: { role: "admin" } }),
}));
vi.mock("@/components/library/use-card-size", () => ({
  useCardSize: () => [160, () => undefined] as const,
}));
vi.mock("@/components/saved-views/PageActionsMenu", () => ({
  PageActionsMenu: () => (
    <button type="button" aria-label="Page actions">
      ⋯
    </button>
  ),
}));
vi.mock("@/components/saved-views/PageHeading", () => ({
  PageHeading: ({ pageName }: { pageName: string }) => <h1>{pageName}</h1>,
}));
vi.mock("@/components/saved-views/LazyRail", () => ({
  LazyRail: () => null,
}));

const { PageRails } = await import("@/components/saved-views/PageRails");

describe("PageRails chrome", () => {
  it("pins card size + page actions under the heading from sm up, and in the phone bar with the title", () => {
    const { container } = render(
      <PageRails
        pageId="page-marvel"
        pageName="Marvel"
        pageDescription="All things Marvel."
        isSystem={false}
        showInSidebar
      />,
    );
    const row = container.querySelector<HTMLElement>("[data-toolbar-row]");
    expect(row).not.toBeNull();
    expect(row!.className).toContain("max-sm:hidden");
    expect(
      within(row!).getByRole("button", { name: "View options" }),
    ).toBeTruthy();
    expect(
      within(row!).getByRole("button", { name: "Page actions" }),
    ).toBeTruthy();
    // The `sm`+ heading is hidden on phones; the bar's h1 takes over.
    const headings = screen.getAllByRole("heading", {
      level: 1,
      name: "Marvel",
    });
    expect(headings.length).toBe(2);
    const bar = container.querySelector<HTMLElement>("[data-phone-controls]");
    expect(bar).not.toBeNull();
    expect(bar!.className).toContain("sticky");
    expect(bar!.className).toContain("top-(--topbar-h)");
    expect(
      within(bar!).getByRole("heading", { level: 1, name: "Marvel" }),
    ).toBeTruthy();
    expect(
      within(bar!).getByRole("button", { name: "View options" }),
    ).toBeTruthy();
    expect(
      within(bar!).getByRole("button", { name: "Page actions" }),
    ).toBeTruthy();
    const desktopHeading = headings.find((h) => !bar!.contains(h))!;
    expect(desktopHeading.parentElement!.className).toContain("max-sm:hidden");
    // Description follows the bar in flow, phone-only.
    const desc = screen.getByText("All things Marvel.");
    expect(desc.className).toContain("sm:hidden");
  });

  it("omits the phone description paragraph when the page has none", () => {
    render(
      <PageRails
        pageId="sys-home"
        pageName="Home"
        pageDescription={null}
        isSystem
        showInSidebar
      />,
    );
    expect(document.querySelector("p.sm\\:hidden")).toBeNull();
  });
});
