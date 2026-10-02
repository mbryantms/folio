// @vitest-environment jsdom
/**
 * WP-7.7 Related tab on the series page: lazy loading, `?tab=` deep link
 * + URL sync, and the tab-label count. The real query hooks run against
 * a mocked `apiFetch`, so the test sees exactly which endpoints fire.
 */
import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";

const calls = vi.hoisted(() => ({ paths: [] as string[] }));

function json(body: unknown) {
  return new Response(JSON.stringify(body), {
    status: 200,
    headers: { "content-type": "application/json" },
  });
}

vi.mock("@/lib/api/auth-refresh", () => ({
  getCsrfToken: () => "csrf",
  apiFetch: async (path: string) => {
    calls.paths.push(path);
    if (path.startsWith("/auth/me")) return json({ id: "u", role: "user" });
    if (path.includes("/relationships"))
      return json({ series_id: "s1", relationships: [], arcs: [], chain: [] });
    if (path.startsWith("/relationship-kinds"))
      return json({ groups: [], kinds: [] });
    return json({ items: [], next_cursor: null, total: 0 });
  },
}));
vi.mock("next/navigation", () => ({
  useRouter: () => ({ push: vi.fn(), replace: vi.fn(), refresh: vi.fn() }),
  usePathname: () => "/series/saga",
  useSearchParams: () => new URLSearchParams(),
}));

import {
  StackedTabsPanel,
  StableTabsPanelStack,
} from "@/components/library/StableTabsPanelStack";
import { SeriesRelatedTab } from "@/components/library/SeriesRelatedTab";
import {
  SeriesTabs,
  resolveSeriesTab,
  urlWithTab,
} from "@/app/[locale]/(library)/series/[slug]/SeriesTabs";

class IOStub {
  observe() {}
  unobserve() {}
  disconnect() {}
  takeRecords() {
    return [];
  }
}

function relatedCalls(): string[] {
  return calls.paths.filter(
    (p) =>
      p.includes("/relationships") ||
      p.includes("/same-universe") ||
      p.includes("/similar"),
  );
}

function renderPage(initialTab: string | null = null, count = 3) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <SeriesTabs
        seriesSlug="saga"
        initialTab={initialTab}
        relationshipCount={count}
        hasAppearances={false}
        hasNotes={false}
      >
        <StableTabsPanelStack>
          <StackedTabsPanel value="credits">credits body</StackedTabsPanel>
          <StackedTabsPanel value="related">
            <SeriesRelatedTab seriesSlug="saga" seriesId="s1" />
          </StackedTabsPanel>
        </StableTabsPanelStack>
      </SeriesTabs>
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  calls.paths = [];
  vi.stubGlobal("IntersectionObserver", IOStub);
  window.history.replaceState(null, "", "/series/saga?q=x");
});

describe("Related tab", () => {
  it("fetches nothing related until the tab is opened", async () => {
    renderPage();
    await act(async () => {
      await new Promise((r) => setTimeout(r, 20));
    });
    expect(screen.getByText("credits body")).toBeTruthy();
    expect(screen.queryByTestId("series-related-tab")).toBeNull();
    expect(relatedCalls()).toEqual([]);

    const trigger = screen.getByRole("tab", { name: /Related/ });
    await act(async () => {
      fireEvent.mouseDown(trigger, { button: 0 });
      fireEvent.click(trigger);
    });
    // Poll rather than sleep: lazy panels mount and fetch asynchronously,
    // and CI runners are slower than a laptop.
    await waitFor(() =>
      expect(screen.getByTestId("series-related-tab")).toBeTruthy(),
    );
    await waitFor(() => {
      const fired = relatedCalls();
      expect(fired.some((p) => p === "/series/saga/relationships")).toBe(true);
      expect(
        fired.some((p) => p.startsWith("/series/saga/same-universe")),
      ).toBe(true);
      expect(fired.some((p) => p.startsWith("/series/saga/similar"))).toBe(
        true,
      );
    });
    // The URL follows the tab, other params kept.
    expect(window.location.search).toBe("?q=x&tab=related");
  });

  it("opens straight on the Related tab from ?tab=related", async () => {
    renderPage("related");
    await waitFor(() =>
      expect(screen.getByTestId("series-related-tab")).toBeTruthy(),
    );
    expect(screen.getByRole("tab", { name: /Related/ }).dataset.state).toBe(
      "active",
    );
  });

  it("labels the tab with the server count, then the live count", async () => {
    renderPage(null, 3);
    const trigger = screen.getByRole("tab", { name: /Related/ });
    expect(trigger.textContent).toBe("Related3");
    await act(async () => {
      fireEvent.mouseDown(trigger, { button: 0 });
    });
    // The loaded relationships (none) replace the server snapshot once the
    // query resolves; poll for it (a fixed 20 ms sleep raced on CI).
    await waitFor(() =>
      expect(screen.getByRole("tab", { name: /Related/ }).textContent).toBe(
        "Related",
      ),
    );
  });
});

describe("tab URL helpers", () => {
  it("resolves only tabs the page renders", () => {
    const available = new Set(["credits", "related"] as const);
    expect(resolveSeriesTab("related", available)).toBe("related");
    expect(resolveSeriesTab("RELATED", available)).toBe("related");
    expect(resolveSeriesTab("markers", available)).toBe("credits");
    expect(resolveSeriesTab(null, available)).toBe("credits");
  });

  it("sets ?tab= and drops it for the default tab", () => {
    expect(urlWithTab("http://x/series/a?q=1", "related")).toBe(
      "http://x/series/a?q=1&tab=related",
    );
    expect(urlWithTab("http://x/series/a?tab=related&q=1", "credits")).toBe(
      "http://x/series/a?q=1",
    );
  });
});
