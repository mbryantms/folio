// @vitest-environment jsdom
/**
 * U-11: a search where every category came back empty is no longer a dead
 * end. The server sends the closest series names the caller can see
 * (`SeriesListView.suggestions`); the `/search` page and the search sheet
 * offer each as a control that runs that search, plus a way into Browse.
 */
import {
  act,
  fireEvent,
  render,
  renderHook,
  screen,
  waitFor,
} from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { SearchNoResults } from "@/components/search/SearchNoResults";

describe("SearchNoResults (the /search page's all-empty state)", () => {
  it("offers each suggestion as a button that runs that search", () => {
    const onPick = vi.fn();
    render(
      <SearchNoResults
        query="sega"
        suggestions={["Saga", "Sea of Stars"]}
        onPick={onPick}
      />,
    );
    expect(screen.getByText(/No matches for/).textContent).toContain("sega");
    const list = screen.getByRole("list", { name: "Did you mean:" });
    expect(list.querySelectorAll("li")).toHaveLength(2);
    const saga = screen.getByRole("button", { name: "Search for Saga" });
    expect(saga.textContent).toBe("Saga");
    saga.focus();
    expect(document.activeElement).toBe(saga);
    fireEvent.click(saga);
    expect(onPick).toHaveBeenCalledWith("Saga");
  });

  it("shows no suggestion block when the server sent none", () => {
    render(
      <SearchNoResults query="zzzzqq" suggestions={[]} onPick={vi.fn()} />,
    );
    expect(screen.queryByText("Did you mean:")).toBeNull();
    expect(screen.queryByRole("list")).toBeNull();
    expect(screen.queryAllByRole("button")).toHaveLength(0);
  });

  it("always links to Browse", () => {
    render(
      <SearchNoResults query="zzzzqq" suggestions={[]} onPick={vi.fn()} />,
    );
    const browse = screen.getByRole("link", { name: "Browse the library" });
    expect(browse.getAttribute("href")).toBe("/browse");
  });

  it("renders a suggested name as text, never as markup", () => {
    render(
      <SearchNoResults
        query="x"
        suggestions={["<b>Bold</b> & co"]}
        onPick={vi.fn()}
      />,
    );
    const btn = screen.getByRole("button", {
      name: "Search for <b>Bold</b> & co",
    });
    expect(btn.textContent).toBe("<b>Bold</b> & co");
    expect(btn.querySelector("b")).toBeNull();
  });
});

// ───── useGlobalSearch: suggestions only once everything is empty ─────

const queries = vi.hoisted(() => ({
  series: {
    data: undefined as unknown,
    isFetching: false,
  },
  issues: { data: { items: [] as unknown[] }, isFetching: false },
}));
const empty = { data: { items: [] }, isFetching: false };
vi.mock("@/lib/api/queries", () => ({
  useSeriesList: () => queries.series,
  useIssueSearch: () => queries.issues,
  useMarkerSearch: () => empty,
  usePeopleSearch: () => empty,
  useEntitySearch: () => empty,
  useSavedViews: () => ({ data: { items: [] } }),
  useCollections: () => ({ data: [] }),
  useMePages: () => ({ data: [] }),
}));

describe("useGlobalSearch suggestions", () => {
  beforeEach(() => {
    queries.series = {
      data: {
        items: [],
        next_cursor: null,
        total: 0,
        suggestions: [{ name: "Saga" }],
      },
      isFetching: false,
    };
    queries.issues = { data: { items: [] }, isFetching: false };
  });

  it("passes the server's names through when every category is empty", async () => {
    const { useGlobalSearch } = await import("@/lib/search/use-search");
    const { result } = renderHook(() => useGlobalSearch("sega"));
    expect(result.current.total).toBe(0);
    expect(result.current.suggestions).toEqual(["Saga"]);
  });

  it("drops them while a backend is still fetching or another category matched", async () => {
    const { useGlobalSearch } = await import("@/lib/search/use-search");
    queries.series.isFetching = true;
    const fetching = renderHook(() => useGlobalSearch("sega"));
    expect(fetching.result.current.suggestions).toEqual([]);

    queries.series.isFetching = false;
    queries.issues = {
      data: {
        items: [
          {
            id: "i1",
            title: "Sega issue",
            number: "1",
            series_name: "Other",
            series_slug: "other",
            slug: "1",
          },
        ],
      },
      isFetching: false,
    };
    const matched = renderHook(() => useGlobalSearch("sega"));
    expect(matched.result.current.total).toBeGreaterThan(0);
    expect(matched.result.current.suggestions).toEqual([]);
  });

  it("is empty when the server sent none", async () => {
    const { useGlobalSearch } = await import("@/lib/search/use-search");
    queries.series = {
      data: { items: [], next_cursor: null, total: 0 },
      isFetching: false,
    };
    const { result } = renderHook(() => useGlobalSearch("zzzzqq"));
    expect(result.current.suggestions).toEqual([]);
  });
});

// ───── the search sheet (phone Search tab, ⌘K) ─────

describe("SearchModal no-results state", () => {
  it("lists the suggestions as items that run the search, with a Browse link", async () => {
    vi.resetModules();
    const calls: string[] = [];
    vi.doMock("next/navigation", () => ({
      useRouter: () => ({ push: vi.fn() }),
      usePathname: () => "/",
    }));
    vi.doMock("@/lib/search/use-search-actions", () => ({
      useSearchActions: () => ({
        actions: [],
        run: vi.fn(),
        detailFor: () => null,
        keyHintFor: () => null,
      }),
    }));
    const { SEARCH_CATEGORIES } = await import("@/lib/search/types");
    const groups = Object.fromEntries(
      SEARCH_CATEGORIES.map((def) => [def.key, []]),
    );
    vi.doMock("@/lib/search/use-search", () => ({
      useGlobalSearch: (q: string) => {
        calls.push(q);
        const enabled = q.length >= 2;
        return {
          enabled,
          isLoading: false,
          groups,
          total: 0,
          suggestions: q === "sega" ? ["Saga"] : [],
        };
      },
    }));
    const { SearchModal } = await import("@/components/SearchModal");
    render(<SearchModal open onOpenChange={vi.fn()} />);

    const input = screen.getByPlaceholderText(/^Search series/);
    fireEvent.change(input, { target: { value: "sega" } });
    const item = await screen.findByRole("option", { name: "Search for Saga" });
    expect(screen.getByText(/No matches for/).textContent).toContain("sega");
    expect(
      screen
        .getByRole("link", { name: "Browse the library" })
        .getAttribute("href"),
    ).toBe("/browse");

    act(() => {
      fireEvent.click(item);
    });
    await waitFor(() => expect(calls.at(-1)).toBe("Saga"));
    expect((input as HTMLInputElement).value).toBe("Saga");
  });

  it("shows the plain message and Browse, no suggestion items, when there are none", async () => {
    const { SearchModal } = await import("@/components/SearchModal");
    render(<SearchModal open onOpenChange={vi.fn()} />);
    const input = screen.getByPlaceholderText(/^Search series/);
    fireEvent.change(input, { target: { value: "zzzzqq" } });
    await screen.findByRole("link", { name: "Browse the library" });
    expect(screen.queryByRole("option")).toBeNull();
    expect(screen.getAllByText(/No matches for/)).toHaveLength(1);
  });
});
