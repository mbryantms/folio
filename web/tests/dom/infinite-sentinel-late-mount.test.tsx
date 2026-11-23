// @vitest-environment jsdom
/**
 * `useInfiniteSentinel` when the sentinel mounts AFTER the infinite query
 * has settled.
 *
 * The CBL saved-view page (`CblViewDetail`) early-returns a skeleton while
 * the list detail loads, and only then renders the entry grid and its
 * sentinel. When the entries query settled first, the old `useRef`-based
 * observer effect ran with no element, and nothing in its deps changed when
 * the sentinel later mounted — so the observer was never attached and the
 * view stopped at the first 100 entries (prod, 2026-11). The hook now hands
 * back a callback ref, so the element mounting re-runs the effect.
 *
 * The stub models a real IntersectionObserver: an observed target that is
 * already in view reports as intersecting straight away.
 */
import { readFileSync } from "node:fs";
import { join } from "node:path";

import {
  QueryClient,
  QueryClientProvider,
  useInfiniteQuery,
  useQuery,
} from "@tanstack/react-query";
import { act, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { InfiniteSentinel } from "@/components/folio/infinite-sentinel";
import { useInfiniteSentinel } from "@/lib/ui/use-infinite-sentinel";

const observed = vi.hoisted(() => ({ count: 0 }));

vi.stubGlobal(
  "IntersectionObserver",
  class {
    private live = true;
    constructor(private cb: IntersectionObserverCallback) {}
    observe(el: Element) {
      observed.count += 1;
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

type Page = { items: string[]; next_cursor: number | null };

const net = vi.hoisted(() => ({ calls: [] as number[] }));

async function fetchPage(n: number): Promise<Page> {
  net.calls.push(n);
  // A real request takes a beat, so `isFetchingNextPage` is seen to flip
  // (an instant resolve lets TanStack batch the true→false away).
  await new Promise((r) => setTimeout(r, 5));
  return { items: [`entry-${n}`], next_cursor: n < 2 ? n + 1 : null };
}

/** Resolves the slow "detail" query on demand. */
let releaseDetail: () => void = () => {};

function useEntries() {
  return useInfiniteQuery({
    queryKey: ["entries"],
    queryFn: ({ pageParam }) => fetchPage(pageParam),
    initialPageParam: 0,
    getNextPageParam: (last: Page) => last.next_cursor ?? undefined,
  });
}

function useSlowDetail() {
  return useQuery({
    queryKey: ["detail"],
    queryFn: () =>
      new Promise<{ name: string }>((resolve) => {
        releaseDetail = () => resolve({ name: "Big list" });
      }),
  });
}

/** CblViewDetail's shape: hooks first, then an early return while the
 *  detail loads, then the grid + sentinel. */
function LateSentinelWithHook() {
  const detail = useSlowDetail();
  const entries = useEntries();
  const sentinelRef = useInfiniteSentinel(entries);
  if (detail.isLoading) return <p>Loading view…</p>;
  const items = entries.data?.pages.flatMap((p) => p.items) ?? [];
  return (
    <div>
      <h1>{detail.data?.name}</h1>
      <ul>
        {items.map((i) => (
          <li key={i}>{i}</li>
        ))}
      </ul>
      <div ref={sentinelRef} className={entries.hasNextPage ? "h-12" : ""} />
    </div>
  );
}

function LateSentinelComponent() {
  const detail = useSlowDetail();
  const entries = useEntries();
  if (detail.isLoading) return <p>Loading view…</p>;
  const items = entries.data?.pages.flatMap((p) => p.items) ?? [];
  return (
    <div>
      <ul>
        {items.map((i) => (
          <li key={i}>{i}</li>
        ))}
      </ul>
      <InfiniteSentinel query={entries} />
    </div>
  );
}

function renderWith(ui: React.ReactElement) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>{ui}</QueryClientProvider>,
  );
}

async function settle() {
  for (let i = 0; i < 10; i++) {
    await act(async () => {
      await new Promise((r) => setTimeout(r, 0));
    });
  }
}

beforeEach(() => {
  net.calls.length = 0;
  observed.count = 0;
});

describe("useInfiniteSentinel with a sentinel that mounts late", () => {
  it.each([
    ["the hook", LateSentinelWithHook],
    ["<InfiniteSentinel>", LateSentinelComponent],
  ])(
    "walks every page once the sentinel mounts (%s)",
    async (_label, Component) => {
      renderWith(<Component />);
      // The entries query settles while the page is still a skeleton.
      await settle();
      expect(net.calls).toEqual([0]);
      expect(screen.getByText("Loading view…")).toBeTruthy();

      // The detail lands; the grid and its sentinel mount.
      await act(async () => releaseDetail());
      expect(await screen.findByText("entry-2")).toBeTruthy();
      await settle();
      expect(net.calls).toEqual([0, 1, 2]);
      expect(observed.count).toBeGreaterThan(0);
    },
  );
});

describe("CblViewDetail", () => {
  it("drives its entry grid through the shared sentinel hook", () => {
    // A hand-rolled `useRef` observer here is how the 100-entry stop
    // shipped; keep the page on the callback-ref hook.
    const src = readFileSync(
      join(process.cwd(), "components/saved-views/CblViewDetail.tsx"),
      "utf8",
    );
    expect(src).toContain("useInfiniteSentinel(entriesQuery)");
    expect(src).not.toMatch(/new IntersectionObserver/);
  });
});
