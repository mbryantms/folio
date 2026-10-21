// @vitest-environment jsdom
/**
 * WP-5.5 — the single "Browse" sidebar destination renders a tabbed index
 * (Characters / Teams / Story arcs / Publishers / Creators), mounts only the
 * active tab, and keeps the active tab in `?tab=`.
 */
import { act, fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

vi.stubGlobal(
  "IntersectionObserver",
  class {
    observe() {}
    disconnect() {}
    unobserve() {}
  },
);

const replace = vi.fn();
vi.mock("next/navigation", () => ({
  useRouter: () => ({ replace, push: vi.fn(), prefetch: vi.fn() }),
  usePathname: () => "/browse",
}));

const entityCalls: string[] = [];
function infinite(items: unknown[]) {
  return {
    data: { pages: [{ items, next_cursor: null, total: items.length }] },
    isLoading: false,
    isError: false,
    hasNextPage: false,
    isFetchingNextPage: false,
    fetchNextPage: vi.fn(async () => undefined),
  };
}
vi.mock("@/lib/api/queries", () => ({
  useEntityListInfinite: (kind: string) => {
    entityCalls.push(kind);
    return infinite([
      {
        id: `${kind}-1`,
        slug: `${kind}-one`,
        name: `${kind} one`,
        series_count: 1,
        issue_count: 2,
      },
    ]);
  },
  useCreatorsInfinite: () =>
    infinite([
      {
        person: "Alan Moore",
        slug: "alan-moore",
        roles: ["writer"],
        credit_count: 3,
      },
    ]),
  usePeopleSearch: () => ({
    data: undefined,
    isLoading: false,
    isError: false,
  }),
}));

import { BrowseTabs } from "@/components/library/BrowseTabs";
import { parseBrowseTab } from "@/components/library/browse-tabs";

describe("BrowseTabs", () => {
  beforeEach(() => {
    entityCalls.length = 0;
    replace.mockClear();
  });

  it("renders one tab per browsable kind, including creators", () => {
    render(<BrowseTabs initialTab="characters" />);
    const names = screen.getAllByRole("tab").map((t) => t.textContent);
    expect(names).toEqual([
      "Characters",
      "Teams",
      "Story arcs",
      "Publishers",
      "Creators",
    ]);
    // Only the active tab is mounted → only one list query fires.
    expect(new Set(entityCalls)).toEqual(new Set(["characters"]));
    expect(
      screen.getByRole("link", { name: /characters one/ }).getAttribute("href"),
    ).toBe("/characters/characters-one");
  });

  it("switching tabs mounts that index and writes ?tab=", async () => {
    render(<BrowseTabs initialTab="characters" />);
    const tab = screen.getByRole("tab", { name: "Story arcs" });
    await act(async () => {
      fireEvent.mouseDown(tab, { button: 0 });
      fireEvent.click(tab);
    });
    expect(replace).toHaveBeenCalledWith("/browse?tab=arcs", { scroll: false });
    expect(
      screen.getByRole("link", { name: /arcs one/ }).getAttribute("href"),
    ).toBe("/arcs/arcs-one");
  });

  it("creators tab reuses the creators index", () => {
    render(<BrowseTabs initialTab="creators" />);
    expect(
      screen.getByRole("link", { name: /Alan Moore/ }).getAttribute("href"),
    ).toBe("/creators/alan-moore");
  });

  it("tab strip scrolls (no flex-wrap) and the index renders both A–Z rails", () => {
    render(<BrowseTabs initialTab="characters" />);
    // `TabsList` is a fixed-height scroll strip; wrapping would clip
    // the second row, so the phone fix is simply not to wrap.
    expect(screen.getByRole("tablist").className).not.toContain("flex-wrap");
    // Desktop strip hidden on phones; phone scrubber (sm:hidden) next
    // to it; the scrubber sits in the 16px right gutter so the grid keeps its width.
    const rails = screen.getAllByRole("navigation", { name: "Jump to letter" });
    expect(rails).toHaveLength(2);
    expect(rails.some((r) => r.className.includes("max-sm:hidden"))).toBe(true);
    expect(rails.some((r) => r.hasAttribute("data-az-scrubber"))).toBe(true);
  });

  it("creators tab renders the scrubber too", () => {
    render(<BrowseTabs initialTab="creators" />);
    const rails = screen.getAllByRole("navigation", { name: "Jump to letter" });
    expect(rails.some((r) => r.hasAttribute("data-az-scrubber"))).toBe(true);
  });

  it("parseBrowseTab falls back to characters", () => {
    expect(parseBrowseTab("teams")).toBe("teams");
    expect(parseBrowseTab("bogus")).toBe("characters");
    expect(parseBrowseTab(undefined)).toBe("characters");
  });
});
