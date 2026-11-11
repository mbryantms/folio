// @vitest-environment jsdom
/**
 * "New in series you're reading": the home rail renders the first page of
 * `/me/new-in-series` (one request) and hides itself when it is empty;
 * the "see all" page walks the cursor through `<InfiniteSentinel>` and
 * shows the first page's `total`.
 */
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

vi.stubGlobal(
  "IntersectionObserver",
  class {
    private live = true;
    constructor(private cb: IntersectionObserverCallback) {}
    observe(el: Element) {
      queueMicrotask(() => {
        if (!this.live) return;
        this.cb(
          [{ isIntersecting: true, target: el } as IntersectionObserverEntry],
          this as unknown as IntersectionObserver,
        );
      });
    }
    unobserve() {}
    disconnect() {
      this.live = false;
    }
  },
);

const net = vi.hoisted(() => ({ calls: [] as string[], empty: false }));

function issue(id: string) {
  return { id, series_id: `s-${id}`, slug: id, series_slug: "s" };
}

vi.mock("@/lib/api/auth-refresh", () => ({
  getCsrfToken: () => null,
  apiFetch: async (path: string) => {
    net.calls.push(path);
    const body = net.empty
      ? { items: [], next_cursor: null, total: 0 }
      : path.includes("cursor=")
        ? { items: [issue("c")], next_cursor: null }
        : { items: [issue("a"), issue("b")], next_cursor: "p2", total: 3 };
    return new Response(JSON.stringify(body), {
      status: 200,
      headers: { "Content-Type": "application/json" },
    });
  },
}));
vi.mock("@/components/library/IssueCard", () => ({
  IssueCard: ({ issue }: { issue: { id: string } }) => (
    <div data-testid="card">{issue.id}</div>
  ),
  IssueCardSkeleton: () => null,
}));
vi.mock("@/components/library/CardSizeOptions", () => ({
  CardSizeOptions: () => null,
  CardSizeFields: () => null,
}));
vi.mock("@/components/library/PhoneControlsBar", () => ({
  PhoneControlsBar: () => null,
}));
vi.mock("@/components/library/use-card-size", () => ({
  useCardSize: () => [180, () => {}],
}));

import { SystemViewDetail } from "@/components/saved-views/SystemViewDetail";
import {
  NewInSeriesRailBody,
  useSystemRailIsEmpty,
} from "@/components/saved-views/system-rails";
import type { SavedViewView } from "@/lib/api/types";

function wrap(node: React.ReactNode) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>{node}</QueryClientProvider>,
  );
}

/** The parent's contract: the rail section only mounts when not empty. */
function Rail() {
  const empty = useSystemRailIsEmpty("new_in_series");
  if (empty) return <p>rail hidden</p>;
  return (
    <section>
      <NewInSeriesRailBody itemStyle={{}} />
    </section>
  );
}

beforeEach(() => {
  net.calls = [];
  net.empty = false;
});

describe("New in series rail", () => {
  it("renders the first page in one request", async () => {
    wrap(<Rail />);
    await waitFor(() =>
      expect(screen.getAllByTestId("card").map((c) => c.textContent)).toEqual([
        "a",
        "b",
      ]),
    );
    expect(net.calls).toEqual(["/me/new-in-series?limit=24"]);
  });

  it("hides itself when empty", async () => {
    net.empty = true;
    wrap(<Rail />);
    expect(await screen.findByText("rail hidden")).toBeTruthy();
  });
});

describe("New in series see-all page", () => {
  const view = {
    id: "00000000-0000-0000-0000-000000000017",
    kind: "system",
    system_key: "new_in_series",
    name: "New in series you're reading",
    description: null,
  } as unknown as SavedViewView;

  it("renders page one, fetches the next on the sentinel, shows the total", async () => {
    wrap(<SystemViewDetail view={view} />);
    expect(await screen.findByText("3 items")).toBeTruthy();
    await waitFor(() =>
      expect(screen.getAllByTestId("card").map((c) => c.textContent)).toEqual([
        "a",
        "b",
        "c",
      ]),
    );
    expect(net.calls).toEqual([
      "/me/new-in-series?limit=24",
      "/me/new-in-series?limit=24&cursor=p2",
    ]);
  });

  it("says so when there is nothing new", async () => {
    net.empty = true;
    wrap(<SystemViewDetail view={view} />);
    expect(await screen.findByText(/Nothing new in the series/)).toBeTruthy();
  });
});
