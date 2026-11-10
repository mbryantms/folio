// @vitest-environment jsdom
/**
 * `useInfiniteSentinel` / `<InfiniteSentinel>` after a failed next page.
 *
 * A real IntersectionObserver reports a target that is already in view as
 * soon as it is observed. The hook rebuilds its observer whenever
 * `isFetchingNextPage` flips, so after a failed page the fresh observer
 * saw the still-visible sentinel and fetched again: a request loop
 * against a failing server. The stub below models that (every `observe`
 * fires an intersecting entry), and the test pins that the hook parks
 * until Retry and resumes once the retried page lands.
 */
import {
  QueryClient,
  QueryClientProvider,
  useInfiniteQuery,
} from "@tanstack/react-query";
import { act, fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { InfiniteSentinel } from "@/components/folio/infinite-sentinel";

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

type Page = { items: string[]; next: number | null };

const net = vi.hoisted(() => ({
  calls: [] as number[],
  failing: new Set<number>(),
}));

async function fetchPage(n: number): Promise<Page> {
  net.calls.push(n);
  if (net.failing.has(n)) throw new Error("HttpError: /things?cursor=1 → 500");
  return { items: [`item-${n}`], next: n < 2 ? n + 1 : null };
}

function List() {
  const query = useInfiniteQuery({
    queryKey: ["things"],
    queryFn: ({ pageParam }) => fetchPage(pageParam),
    initialPageParam: 0,
    getNextPageParam: (last: Page) => last.next,
  });
  const items = query.data?.pages.flatMap((p) => p.items) ?? [];
  return (
    <div>
      <ul>
        {items.map((i) => (
          <li key={i}>{i}</li>
        ))}
      </ul>
      <InfiniteSentinel query={query} />
    </div>
  );
}

function renderList() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <List />
    </QueryClientProvider>,
  );
}

async function settle() {
  // Several macrotask turns: long enough for a looping observer to have
  // re-fired many times.
  for (let i = 0; i < 10; i++) {
    await act(async () => {
      await new Promise((r) => setTimeout(r, 0));
    });
  }
}

beforeEach(() => {
  net.calls.length = 0;
  net.failing.clear();
});

describe("useInfiniteSentinel after a failed next page", () => {
  it("stops fetching until Retry, then resumes", async () => {
    net.failing.add(1);
    renderList();

    expect(await screen.findByText("Couldn't load more.")).toBeTruthy();
    await settle();
    // Page 0 once, page 1 once: the failure did not re-trigger the observer.
    expect(net.calls).toEqual([0, 1]);
    // Loaded rows stay; no raw error text reaches the page.
    expect(screen.getByText("item-0")).toBeTruthy();
    expect(screen.queryByText(/HttpError|500/)).toBeNull();

    net.failing.delete(1);
    fireEvent.click(screen.getByRole("button", { name: "Retry" }));

    // The retried page lands, then the observer resumes and walks on.
    expect(await screen.findByText("item-2")).toBeTruthy();
    await settle();
    expect(net.calls).toEqual([0, 1, 1, 2]);
    expect(screen.queryByText("Couldn't load more.")).toBeNull();
  });

  it("parks again when the retry fails too", async () => {
    net.failing.add(1);
    renderList();
    await screen.findByText("Couldn't load more.");

    fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    await settle();
    expect(await screen.findByText("Couldn't load more.")).toBeTruthy();
    expect(net.calls).toEqual([0, 1, 1]);
  });
});
