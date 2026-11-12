// @vitest-environment jsdom
/**
 * The cached card lists are the one copy of a card's progress. A mark
 * patches them in place: the patch survives an infinite list's next-page
 * fetch, an in-flight list fetch can't land on top of it, overlapping
 * writes roll back only what is still their own, and a series-level mark
 * resets the issue page to the server's state.
 */
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  act,
  fireEvent,
  render,
  renderHook,
  screen,
  waitFor,
} from "@testing-library/react";
import type { ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const net = vi.hoisted(() => ({
  calls: [] as string[],
  /** Pages of the series issue list, by cursor ("" = first). */
  pages: new Map<string, { items: unknown[]; next_cursor: string | null }>(),
  /** When set, list GETs wait on it before answering. */
  gate: null as Promise<void> | null,
  issueRecord: null as unknown,
}));

vi.mock("@/lib/api/auth-refresh", () => ({
  getCsrfToken: () => null,
  apiFetch: async (path: string, init?: RequestInit) => {
    const method = init?.method ?? "GET";
    net.calls.push(`${method} ${path}`);
    const json = (body: unknown) =>
      new Response(JSON.stringify(body), {
        status: 200,
        headers: { "Content-Type": "application/json" },
      });
    if (method !== "GET") return json({ updated: 1 });
    if (path.startsWith("/progress?issue_id=")) {
      return json({ records: net.issueRecord ? [net.issueRecord] : [] });
    }
    // Snapshot the page now: what the server had when the request began.
    const cursor = new URL(path, "http://x").searchParams.get("cursor") ?? "";
    const page = structuredClone(net.pages.get(cursor));
    if (net.gate) await net.gate;
    return json(page);
  },
}));
vi.mock("next/navigation", () => ({
  useRouter: () => ({ push: () => {} }),
}));
vi.mock("@/lib/visual-effects", () => ({
  useVisualEffect: () => false,
  useReducedMotion: () => true,
}));
vi.mock("@/components/CoverLongPressActions", () => ({
  useCoverLongPressActions: () => ({ wrapperProps: {}, sheet: null }),
}));
vi.mock("@/components/collections/useCoverMenuCollectionActions", () => ({
  useCoverMenuCollectionActions: () => ({ actions: [], dialog: null }),
}));
vi.mock("@/components/CoverMenuButton", () => ({
  CoverMenuButton: () => null,
}));
vi.mock("@/components/library/PageScrub", () => ({
  PageScrub: () => null,
}));
vi.mock("@/lib/ui/use-share-link", () => ({
  useShareLink: () => ({ label: "Share", shareOrCopy: () => {} }),
}));

import { IssueCard } from "@/components/library/IssueCard";
import {
  applyOptimisticProgress,
  rollbackProgress,
  settleProgress,
  useIssueReadState,
} from "@/lib/api/mutations/progress";
import { invalidateIssueCards } from "@/lib/api/mutations/rails";
import { useSeriesIssuesInfinite } from "@/lib/api/queries";
import { queryKeys } from "@/lib/api/query-keys";
import type { ProgressView } from "@/lib/api/types";

const T0 = "2026-11-01T10:00:00+00:00";

function card(id: string, progress: unknown) {
  return {
    id,
    slug: id,
    series_id: "s1",
    series_slug: "saga",
    series_name: "Saga",
    title: `Issue ${id}`,
    number: id,
    page_count: 20,
    state: "active",
    cover_url: null,
    created_at: T0,
    updated_at: T0,
    progress,
  };
}

function readCheck(id: string): Element | null {
  return screen
    .getByRole("link", { name: new RegExp(`Issue ${id}`) })
    .querySelector('[role="img"][aria-label="Read"]');
}

function SeriesList() {
  const q = useSeriesIssuesInfinite("s1");
  return (
    <div>
      {q.data?.pages.flatMap((p) =>
        p.items.map((issue) => <IssueCard key={issue.id} issue={issue} />),
      )}
      <button type="button" onClick={() => void q.fetchNextPage()}>
        More
      </button>
    </div>
  );
}

function newClient() {
  return new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
}

async function mount(expectLinks: number) {
  const client = newClient();
  render(
    <QueryClientProvider client={client}>
      <SeriesList />
    </QueryClientProvider>,
  );
  await waitFor(() =>
    expect(screen.getAllByRole("link").length).toBe(expectLinks),
  );
  return client;
}

beforeEach(() => {
  net.calls.length = 0;
  net.gate = null;
  net.issueRecord = null;
  net.pages = new Map([
    ["", { items: [card("a", null), card("b", null)], next_cursor: "p2" }],
    ["p2", { items: [card("c", null)], next_cursor: null }],
  ]);
});

describe("card-list patches", () => {
  it("a mark on page 1 survives fetching page 2", async () => {
    const client = await mount(2);
    await act(async () => {
      await applyOptimisticProgress(client, [
        { issue_id: "a", page: 0, finished: true },
      ]);
    });
    await waitFor(() => expect(readCheck("a")).not.toBeNull());
    // The server hasn't committed yet: page 1 there still says unread.
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "More" }));
    });
    await waitFor(() => expect(screen.getAllByRole("link").length).toBe(3));
    expect(readCheck("a")).not.toBeNull();
    expect(readCheck("c")).toBeNull();
    // Only page 2 was fetched — page 1 kept its patched copy.
    expect(
      net.calls.filter((c) => c.startsWith("GET /series/s1/issues")),
    ).toHaveLength(2);
  });

  it("a list fetch in flight when a mark is made does not overwrite it", async () => {
    const client = await mount(2);
    let open!: () => void;
    net.gate = new Promise((r) => (open = r));
    // A refetch starts (the server still has "a" unread)…
    const refetch = client
      .refetchQueries({ queryKey: queryKeys.seriesIssues("s1", {}) })
      .catch(() => {});
    await waitFor(() =>
      expect(
        net.calls.filter((c) => c.startsWith("GET /series/s1/issues")).length,
      ).toBeGreaterThan(1),
    );
    // …then the mark, then the old response lands.
    await act(async () => {
      await applyOptimisticProgress(client, [
        { issue_id: "a", page: 0, finished: true },
      ]);
    });
    await act(async () => {
      open();
      await refetch;
      await new Promise((r) => setTimeout(r, 20));
    });
    expect(readCheck("a")).not.toBeNull();
  });

  it("a failed bulk mark keeps a single mark that succeeded in between", async () => {
    const client = await mount(2);
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "More" }));
    });
    await waitFor(() => expect(screen.getAllByRole("link").length).toBe(3));

    // Bulk mark A, B, C read (in flight)…
    let bulk!: Awaited<ReturnType<typeof applyOptimisticProgress>>;
    await act(async () => {
      bulk = await applyOptimisticProgress(
        client,
        ["a", "b", "c"].map((issue_id) => ({
          issue_id,
          page: 0,
          finished: true,
        })),
      );
    });
    // …a single mark of A to mid-issue page 5 is made and succeeds…
    const reply: ProgressView = {
      run: 0,
      issue_id: "a",
      page: 5,
      finished: false,
      percent: 0.25,
      updated_at: T0,
    };
    await act(async () => {
      const single = await applyOptimisticProgress(client, [
        { issue_id: "a", page: 5, finished: false },
      ]);
      settleProgress(client, single, reply);
    });
    // …then the bulk mark fails.
    act(() => rollbackProgress(client, bulk));

    await waitFor(() => expect(readCheck("b")).toBeNull());
    expect(readCheck("c")).toBeNull();
    expect(readCheck("a")).toBeNull();
    expect(
      screen
        .getByRole("link", { name: /Issue a/ })
        .querySelector<HTMLElement>('[style*="width"]')?.style.width,
    ).toBe("25%");
    expect(client.getQueryData(queryKeys.issueProgress("a"))).toMatchObject({
      page: 5,
      percent: 0.25,
    });
    expect(client.getQueryData(queryKeys.issueProgress("b"))).toBeUndefined();
  });
});

describe("issue page", () => {
  it("shows the server's state after a series-level mark, not an earlier write", async () => {
    const client = newClient();
    const wrapper = ({ children }: { children: ReactNode }) => (
      <QueryClientProvider client={client}>{children}</QueryClientProvider>
    );
    const issue = { id: "a", page_count: 20 };
    // The server render: the series was marked read.
    const { result } = renderHook(() => useIssueReadState(issue, "finished"), {
      wrapper,
    });
    // An earlier write from the reader put "page 10" in the per-issue
    // record.
    await act(async () => {
      await applyOptimisticProgress(client, [
        { issue_id: "a", page: 10, finished: false },
      ]);
    });
    await waitFor(() => expect(result.current).toBe("in_progress"));
    // The series-level mark resets the per-issue state (it has no
    // per-issue reply), so the page shows the server's state.
    act(() => invalidateIssueCards(client, { refetchActive: true }));
    await waitFor(() => expect(result.current).toBe("finished"));
    expect(net.calls).toEqual([]);
  });
});
