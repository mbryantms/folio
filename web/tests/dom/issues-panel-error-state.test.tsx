// @vitest-environment jsdom
/**
 * Series page `<IssuesPanel>` when the issue list fails: a failed fetch is
 * not an empty series. A first-page failure shows one message and Retry
 * (never the raw error, never "No issues yet"); a later page failing keeps
 * the loaded issues and offers an inline Retry instead of refetching in a
 * loop; only a successful empty response shows the empty state.
 *
 * Runs the real `useSeriesIssuesInfinite` against a mocked `apiFetch`.
 */
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const api = vi.hoisted(() => ({ fetch: vi.fn() }));
vi.mock("@/lib/api/auth-refresh", async (importOriginal) => ({
  ...(await importOriginal<Record<string, unknown>>()),
  apiFetch: api.fetch,
}));
vi.mock("@/lib/api/queries", async (importOriginal) => ({
  ...(await importOriginal<Record<string, unknown>>()),
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
  usePathname: () => "/series/saga",
}));
vi.mock("next/dynamic", () => ({ default: () => () => null }));
vi.mock("sonner", () => ({ toast: { info: vi.fn(), success: vi.fn() } }));
vi.mock("@/components/collections/BulkAddToCollectionDialog", () => ({
  BulkAddToCollectionDialog: () => null,
}));
vi.mock("@/components/library/IssueCard", () => ({
  IssueCard: ({ issue }: { issue: { number: string } }) => (
    <div data-testid="issue-card">#{issue.number}</div>
  ),
  IssueCardSkeleton: () => null,
}));
// The virtualizer measures layout jsdom does not have; render every card.
vi.mock("@/components/library/VirtualizedCardGrid", () => ({
  VirtualizedCardGrid: ({
    items,
    renderCard,
  }: {
    items: unknown[];
    renderCard: (item: unknown) => React.ReactNode;
  }) => (
    <div>
      {items.map((item, i) => (
        <div key={i}>{renderCard(item)}</div>
      ))}
    </div>
  ),
}));

import { IssuesPanel } from "@/app/[locale]/(library)/series/[slug]/IssuesPanel";

/** Sentinel observers, so a test can scroll the sentinel into view. */
let observers: { cb: IntersectionObserverCallback; on: boolean }[] = [];
class IOStub {
  entry: { cb: IntersectionObserverCallback; on: boolean };
  constructor(cb: IntersectionObserverCallback) {
    this.entry = { cb, on: false };
    observers.push(this.entry);
  }
  observe() {
    this.entry.on = true;
  }
  unobserve() {}
  disconnect() {
    this.entry.on = false;
  }
}
function scrollSentinelIntoView() {
  for (const o of observers.filter((x) => x.on)) {
    o.cb(
      [{ isIntersecting: true } as IntersectionObserverEntry],
      {} as IntersectionObserver,
    );
  }
}

function issue(n: number) {
  return {
    id: `i${n}`,
    slug: String(n),
    series_id: "s1",
    series_slug: "saga",
    title: null,
    number: String(n),
    sort_number: n,
    page_count: 20,
    state: "active",
    cover_url: null,
    read_progress: null,
  };
}
const ok = (body: unknown) =>
  new Response(JSON.stringify(body), {
    status: 200,
    headers: { "Content-Type": "application/json" },
  });
const boom = () =>
  new Response(JSON.stringify({ error: { code: "x", message: "boom" } }), {
    status: 500,
    headers: { "Content-Type": "application/json" },
  });
const EMPTY = { items: [], total: 0, next_cursor: null };

/** Routes `/series/saga/issues?…` calls: `main` answers the main-run
 *  query (with the cursor, if any); specials always answer empty. */
function route(main: (cursor: string | null) => Response) {
  api.fetch.mockImplementation(async (path: string) => {
    const url = new URL(path, "http://x");
    if (url.searchParams.get("kind") === "special") return ok(EMPTY);
    return main(url.searchParams.get("cursor"));
  });
}

function renderPanel() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <IssuesPanel seriesSlug="saga" issueCount={6} />
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  observers = [];
  api.fetch.mockReset();
  vi.stubGlobal("IntersectionObserver", IOStub);
});

describe("IssuesPanel error state", () => {
  it("shows a message and Retry, not the empty state, when the first page fails; Retry loads the issues", async () => {
    let fail = true;
    route(() =>
      fail
        ? boom()
        : ok({ items: [issue(1), issue(2)], total: 2, next_cursor: null }),
    );
    renderPanel();

    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("Couldn't load issues.");
    expect(screen.queryByText("No issues yet")).toBeNull();
    expect(document.body.textContent).not.toMatch(/HttpError|boom/);

    fail = false;
    fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    await waitFor(() =>
      expect(screen.getAllByTestId("issue-card")).toHaveLength(2),
    );
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("shows the empty state when the request succeeds with nothing", async () => {
    route(() => ok(EMPTY));
    renderPanel();
    expect(await screen.findByText("No issues yet")).toBeTruthy();
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("keeps the loaded issues and offers an inline Retry when a later page fails", async () => {
    let pageTwoFails = true;
    route((cursor) => {
      if (!cursor)
        return ok({ items: [issue(1), issue(2)], total: 4, next_cursor: "c2" });
      return pageTwoFails
        ? boom()
        : ok({ items: [issue(3), issue(4)], total: 4, next_cursor: null });
    });
    renderPanel();
    await waitFor(() =>
      expect(screen.getAllByTestId("issue-card")).toHaveLength(2),
    );

    scrollSentinelIntoView();
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("Couldn't load more issues.");
    expect(screen.getAllByTestId("issue-card")).toHaveLength(2);
    expect(screen.queryByText("No issues yet")).toBeNull();

    // A failed page parks the sentinel: scrolling again does not refetch.
    const callsAfterFailure = api.fetch.mock.calls.length;
    scrollSentinelIntoView();
    expect(api.fetch.mock.calls.length).toBe(callsAfterFailure);

    pageTwoFails = false;
    fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    await waitFor(() =>
      expect(screen.getAllByTestId("issue-card")).toHaveLength(4),
    );
    expect(screen.queryByRole("alert")).toBeNull();
  });
});
