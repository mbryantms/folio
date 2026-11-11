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

const sheet = vi.hoisted(() => ({
  push: vi.fn(),
  runAction: vi.fn(),
  calls: [] as string[],
  actions: [] as unknown[],
}));

/** Load SearchModal against a fake `useGlobalSearch`: "sega" finds nothing
 *  and suggests "Saga"; "zzzzqq" finds nothing; "saga" finds one series. */
async function loadSheet() {
  vi.resetModules();
  vi.doMock("next/navigation", () => ({
    useRouter: () => ({ push: sheet.push }),
    usePathname: () => "/",
  }));
  vi.doMock("@/lib/search/use-search-actions", () => ({
    useSearchActions: () => ({
      actions: sheet.actions,
      run: sheet.runAction,
      detailFor: () => null,
      keyHintFor: () => null,
    }),
  }));
  const { SEARCH_CATEGORIES } = await import("@/lib/search/types");
  const empty = Object.fromEntries(
    SEARCH_CATEGORIES.map((def) => [def.key, []]),
  );
  vi.doMock("@/lib/search/use-search", () => ({
    useGlobalSearch: (q: string) => {
      sheet.calls.push(q);
      const hit = q === "saga";
      return {
        enabled: q.length >= 2,
        isLoading: false,
        groups: hit
          ? {
              ...empty,
              series: [
                {
                  kind: "series",
                  id: "s1",
                  title: "Saga",
                  href: "/series/saga",
                },
              ],
            }
          : empty,
        total: hit ? 1 : 0,
        suggestions: q === "sega" ? ["Saga"] : [],
      };
    },
  }));
  const { SearchModal } = await import("@/components/SearchModal");
  render(<SearchModal open onOpenChange={vi.fn()} />);
  return screen.getByPlaceholderText(/^Search series/) as HTMLInputElement;
}

describe("SearchModal no-results state", () => {
  beforeEach(() => {
    sheet.push.mockClear();
    sheet.runAction.mockClear();
    sheet.calls.length = 0;
    sheet.actions = [];
  });

  it("lists the suggestions with nothing highlighted, and a Browse link", async () => {
    const input = await loadSheet();
    fireEvent.change(input, { target: { value: "sega" } });
    const item = await screen.findByRole("option", { name: "Search for Saga" });
    expect(screen.getByText(/No matches for/).textContent).toContain("sega");
    expect(
      screen
        .getByRole("link", { name: "Browse the library" })
        .getAttribute("href"),
    ).toBe("/browse");
    // cmdk would highlight the first item on its own; not a suggestion.
    await waitFor(() =>
      expect(item.getAttribute("aria-selected")).toBe("false"),
    );
  });

  it("plain Enter with only suggestions opens the full search page", async () => {
    const input = await loadSheet();
    fireEvent.change(input, { target: { value: "sega" } });
    await screen.findByRole("option", { name: "Search for Saga" });
    fireEvent.keyDown(input, { key: "Enter" });
    expect(sheet.push).toHaveBeenCalledWith("/search?q=sega");
    // The query was not swapped for the suggestion.
    expect(sheet.calls).not.toContain("Saga");
  });

  it("hovering a suggestion does not make Enter pick it", async () => {
    const input = await loadSheet();
    fireEvent.change(input, { target: { value: "sega" } });
    const item = await screen.findByRole("option", { name: "Search for Saga" });
    fireEvent.pointerMove(item);
    expect(item.getAttribute("aria-selected")).toBe("false");
    fireEvent.keyDown(input, { key: "Enter" });
    expect(sheet.push).toHaveBeenCalledWith("/search?q=sega");
  });

  it("ArrowDown then Enter runs the suggested search", async () => {
    const input = await loadSheet();
    fireEvent.change(input, { target: { value: "sega" } });
    const item = await screen.findByRole("option", { name: "Search for Saga" });
    fireEvent.keyDown(input, { key: "ArrowDown" });
    await waitFor(() =>
      expect(item.getAttribute("aria-selected")).toBe("true"),
    );
    act(() => {
      fireEvent.keyDown(input, { key: "Enter" });
    });
    await waitFor(() => expect(sheet.calls.at(-1)).toBe("Saga"));
    expect(input.value).toBe("Saga");
    expect(sheet.push).not.toHaveBeenCalled();
  });

  it("clicking a suggestion runs that search", async () => {
    const input = await loadSheet();
    fireEvent.change(input, { target: { value: "sega" } });
    const item = await screen.findByRole("option", { name: "Search for Saga" });
    act(() => {
      fireEvent.click(item);
    });
    await waitFor(() => expect(sheet.calls.at(-1)).toBe("Saga"));
    expect(input.value).toBe("Saga");
  });

  it("a real result keeps the first-item highlight and Enter opens it", async () => {
    const input = await loadSheet();
    fireEvent.change(input, { target: { value: "saga" } });
    const hit = await screen.findByRole("option", { name: /Saga/ });
    await waitFor(() => expect(hit.getAttribute("aria-selected")).toBe("true"));
    fireEvent.keyDown(input, { key: "Enter" });
    await waitFor(() =>
      expect(sheet.push).toHaveBeenCalledWith("/series/saga"),
    );
  });

  it("shows the plain message and Browse, no items, when there are no suggestions", async () => {
    const input = await loadSheet();
    fireEvent.change(input, { target: { value: "zzzzqq" } });
    await screen.findByRole("link", { name: "Browse the library" });
    expect(screen.queryByRole("option")).toBeNull();
    expect(screen.getAllByText(/No matches for/)).toHaveLength(1);
    fireEvent.keyDown(input, { key: "Enter" });
    expect(sheet.push).toHaveBeenCalledWith("/search?q=zzzzqq");
  });

  it("after an arrow key, hovering a suggestion still does not make Enter pick it", async () => {
    const { Sun, Moon } = await import("lucide-react");
    sheet.actions = [
      { id: "a1", label: "Toggle theme", group: "Commands", icon: Sun },
      { id: "a2", label: "Night mode", group: "Commands", icon: Moon },
    ];
    const input = await loadSheet();
    fireEvent.change(input, { target: { value: "sega" } });
    const item = await screen.findByRole("option", { name: "Search for Saga" });
    const second = screen.getByRole("option", { name: /Night mode/ });
    // Arrow among the "Jump to…" actions…
    fireEvent.keyDown(input, { key: "ArrowDown" });
    await waitFor(() =>
      expect(second.getAttribute("aria-selected")).toBe("true"),
    );
    await act(async () => {});
    // …then the mouse moves onto the suggestion.
    fireEvent.pointerMove(item);
    expect(item.getAttribute("aria-selected")).toBe("false");
    fireEvent.keyDown(input, { key: "Enter" });
    expect(sheet.push).toHaveBeenCalledWith("/search?q=sega");
    expect(sheet.calls).not.toContain("Saga");
    expect(sheet.runAction).not.toHaveBeenCalled();
  });

  it("after an arrow key, new zero-hit results from a paste are not highlighted", async () => {
    const input = await loadSheet();
    fireEvent.change(input, { target: { value: "saga" } });
    const hit = await screen.findByRole("option", { name: /Saga/ });
    await waitFor(() => expect(hit.getAttribute("aria-selected")).toBe("true"));
    fireEvent.keyDown(input, { key: "ArrowDown" });
    await act(async () => {});
    // A paste from the context menu: a new value, no keydown.
    fireEvent.change(input, { target: { value: "sega" } });
    const item = await screen.findByRole("option", { name: "Search for Saga" });
    await waitFor(() =>
      expect(item.getAttribute("aria-selected")).toBe("false"),
    );
    fireEvent.keyDown(input, { key: "Enter" });
    expect(sheet.push).toHaveBeenCalledWith("/search?q=sega");
    expect(sheet.calls).not.toContain("Saga");
  });

  it("the Enter that commits an IME composition does nothing", async () => {
    const input = await loadSheet();
    fireEvent.change(input, { target: { value: "zzzzqq" } });
    await screen.findByRole("link", { name: "Browse the library" });
    fireEvent.keyDown(input, { key: "Enter", isComposing: true });
    fireEvent.keyDown(input, { key: "Enter", keyCode: 229 });
    expect(sheet.push).not.toHaveBeenCalled();
    fireEvent.keyDown(input, { key: "Enter" });
    expect(sheet.push).toHaveBeenCalledWith("/search?q=zzzzqq");
  });
});
