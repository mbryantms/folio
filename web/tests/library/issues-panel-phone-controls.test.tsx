// @vitest-environment jsdom
/**
 * Series page `<IssuesPanel>` on phones: one `<PhoneControlsBar>`
 * carries the "Issues" h2 in its leading slot plus search / sort / View
 * options, instead of the desktop input + Sort + card-size + Select
 * cluster, which lives in a `StickyToolbarRow` (`max-sm:hidden`). The
 * bar is sticky (phone-layout rule 3: rows with search or sort pin at
 * every width; mid-page it engages once scrolled to) and goes away in
 * select mode, where the sticky `SelectionToolbar` takes over. Its sort
 * sheet drives the same query as the desktop dropdown.
 */
import { fireEvent, render, screen, within } from "@testing-library/react";
import { beforeAll, describe, expect, it, vi } from "vitest";

import type { IssueListFilters } from "@/lib/api/queries";

const seen: IssueListFilters[] = [];
vi.mock("@/lib/api/queries", async (importOriginal) => ({
  ...(await importOriginal<Record<string, unknown>>()),
  useSeriesIssuesInfinite: (_slug: string, filters: IssueListFilters) => {
    seen.push(filters);
    return {
      data: { pages: [{ items: [], total: 0 }] },
      isLoading: false,
      isError: false,
      error: null,
      hasNextPage: false,
      isFetchingNextPage: false,
      fetchNextPage: vi.fn(),
    };
  },
  useMe: () => ({ data: { role: "user" } }),
}));
vi.mock("@/lib/api/mutations", async (importOriginal) => {
  const mutation = () => ({ mutate: vi.fn(), isPending: false });
  return {
    ...(await importOriginal<Record<string, unknown>>()),
    useBulkMarkProgress: mutation,
    useBulkMarkSeriesMatchingProgress: mutation,
    useCreateSeriesSelectionBatch: mutation,
    useSetSeriesCover: mutation,
  };
});
vi.mock("next/navigation", () => ({
  useRouter: () => ({ push: vi.fn(), refresh: vi.fn() }),
}));
vi.mock("next/dynamic", () => ({ default: () => () => null }));
vi.mock("sonner", () => ({ toast: { info: vi.fn(), success: vi.fn() } }));
vi.mock("@/components/collections/BulkAddToCollectionDialog", () => ({
  BulkAddToCollectionDialog: () => null,
}));

import { IssuesPanel } from "@/app/[locale]/(library)/series/[slug]/IssuesPanel";

function mainFilters(): IssueListFilters | undefined {
  return [...seen].reverse().find((f) => f.kind === "main" || "q" in f);
}

describe("IssuesPanel phone controls", () => {
  beforeAll(() => {
    // The load-more sentinel observes itself on mount; jsdom has no IO.
    class IOStub {
      observe() {}
      unobserve() {}
      disconnect() {}
    }
    vi.stubGlobal("IntersectionObserver", IOStub);
  });

  it("carries the heading with search / sort / View options in one sticky bar and hides the desktop cluster below sm", () => {
    const { container } = render(
      <IssuesPanel seriesSlug="saga" issueCount={12} />,
    );
    const bar = container.querySelector<HTMLElement>("[data-phone-controls]");
    expect(bar).not.toBeNull();
    expect(bar!.className).toContain("sm:hidden");
    // The h2 rides in the bar's leading slot; the desktop heading row
    // (its own h2 + count) is `max-sm:hidden`, so jsdom sees both.
    const barHeading = within(bar!).getByRole("heading", {
      level: 2,
      name: /Issues/,
    });
    expect(barHeading.textContent).toContain("12 issues");
    const desktopHeading = screen
      .getAllByRole("heading", { level: 2, name: /Issues/ })
      .find((h) => h !== barHeading)!;
    expect(desktopHeading.parentElement!.parentElement!.className).toContain(
      "max-sm:hidden",
    );
    // Pinned under the topbar once scrolled to (rule 3).
    expect(bar!.className).toContain("sticky");
    expect(bar!.className).toContain("top-(--topbar-h)");
    expect(
      screen.getByRole("button", { name: "Search issues in this series" }),
    ).toBeTruthy();
    expect(
      screen.getByRole("button", { name: "Sort: Issue number" }),
    ).toBeTruthy();
    // Scoped: the desktop card-size popover trigger shares the name.
    expect(
      within(bar!).getByRole("button", { name: "View options" }),
    ).toBeTruthy();

    // The desktop cluster is still rendered (sm+ shows it) but hidden on
    // phones, so the two compositions never both paint.
    const desktopSearch = screen.getByRole("searchbox", {
      name: "Search issues in this series",
    });
    const row = desktopSearch.parentElement!;
    expect(row.className).toContain("max-sm:hidden");
    expect(row.hasAttribute("data-toolbar-row")).toBe(true);
  });

  it("drives the issue query from the phone sort sheet", () => {
    render(<IssuesPanel seriesSlug="saga" issueCount={12} />);
    expect(mainFilters()).toMatchObject({ sort: "number", order: "asc" });

    fireEvent.click(screen.getByRole("button", { name: "Sort: Issue number" }));
    fireEvent.click(screen.getByRole("button", { name: "Descending" }));
    fireEvent.click(screen.getByRole("radio", { name: "Date added" }));
    expect(mainFilters()).toMatchObject({ sort: "created_at", order: "desc" });
    expect(
      screen.getByRole("button", { name: "Sort: Date added" }),
    ).toBeTruthy();
  });

  it("⋯ → Select enters select mode and the bar yields to the selection toolbar", () => {
    const { container } = render(
      <IssuesPanel seriesSlug="saga" issueCount={12} />,
    );
    const bar = container.querySelector<HTMLElement>("[data-phone-controls]")!;
    fireEvent.click(within(bar).getByRole("button", { name: "View options" }));
    const sheet = screen.getByRole("dialog", { name: "View options" });
    // Card size lives in the sheet too.
    expect(within(sheet).getByText("Card size")).toBeTruthy();
    fireEvent.click(within(sheet).getByRole("button", { name: "Select" }));

    expect(container.querySelector("[data-phone-controls]")).toBeNull();
    // The desktop toggle mirrors the mode (it reads "Cancel" while on).
    expect(
      screen
        .getByRole("button", { name: "Cancel" })
        .getAttribute("aria-pressed"),
    ).toBe("true");
  });
});
