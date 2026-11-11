// @vitest-environment jsdom
/**
 * D18: what the home page dehydrates on the server must be what the client
 * hooks read. Prefetch with a fake `apiGet`, hand the state to the page's
 * `<PrefetchBoundary>` under the app's client defaults, mount the real
 * hooks, and assert they render the data without a single browser request
 * — including when the shell created an empty entry for the same key
 * first (the topbar search's disabled `useContinueReading`).
 *
 * The payload crosses as RSC Flight would carry it. Flight keeps
 * `undefined` (an infinite query's `pageParams: [undefined]`), which
 * `JSON.parse(JSON.stringify())` would turn into `null`; `structuredClone`
 * keeps it, so that is the stand-in here.
 */
import {
  QueryClient,
  QueryClientProvider,
  type DehydratedState,
} from "@tanstack/react-query";
import { render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import {
  PrefetchBoundary,
  SNAPSHOT_FRESH_MS,
} from "@/components/PrefetchBoundary";
import { queryKeys } from "@/lib/api/query-keys";
import {
  useCblListWindowInfinite,
  useContinueReading,
  useLibraryList,
  useNewInSeriesInfinite,
  useSavedViews,
} from "@/lib/api/queries";
import type { ContinueReadingView } from "@/lib/api/types";
import {
  createServerQueryClient,
  prefetchPageRails,
} from "@/lib/saved-views/prefetch-page-rails";
import { CBL_RAIL_WINDOW } from "@/lib/saved-views/rail-queries";

const net = vi.hoisted(() => ({ urls: [] as string[] }));
vi.mock("@/lib/api/auth-refresh", () => ({
  apiFetch: vi.fn(async (path: string) => {
    net.urls.push(path);
    return new Response("{}", { status: 500 });
  }),
  getCsrfToken: () => "csrf",
}));
vi.mock("next/navigation", () => ({ usePathname: () => "/" }));

const ROUTES: Record<string, unknown> = {
  "/me/saved-views?pinned_on=home": {
    items: [
      {
        id: "s1",
        name: "Continue reading",
        kind: "system",
        system_key: "continue_reading",
      },
    ],
  },
  "/libraries": [{ id: "lib-1", name: "Comics" }],
  "/me/continue-reading": {
    items: [{ issue: { id: "i1" } }, { issue: { id: "i2" } }],
  },
};
const serverFetch = async <T,>(path: string): Promise<T> => {
  if (!(path in ROUTES)) throw new Error(`404 ${path}`);
  return ROUTES[path] as T;
};

/** The wire: what Flight preserves (undefined included). */
const flight = (state: DehydratedState): DehydratedState =>
  structuredClone(state);

let ids = 0;
const freshId = () => `snapshot-${++ids}`;

function Consumers() {
  const pinned = useSavedViews({ pinnedOn: "home" });
  const cr = useContinueReading();
  const libs = useLibraryList();
  return (
    <p>
      pinned={pinned.data?.items.length ?? "none"} continue=
      {cr.data?.items.length ?? "none"} libraries=
      {libs.data?.length ?? "none"}
    </p>
  );
}

/** The app's browser defaults (components/QueryProvider.tsx). */
const browserClient = () =>
  new QueryClient({
    defaultOptions: { queries: { staleTime: 30_000, retry: false } },
  });

async function homeSnapshot() {
  return prefetchPageRails(createServerQueryClient(), {
    pageId: "home",
    isSystem: true,
    fetch: serverFetch,
  });
}

afterEach(() => {
  vi.useRealTimers();
  net.urls.length = 0;
});

describe("home prefetch → PrefetchBoundary", () => {
  it("client hooks start with the prefetched data and send no request", async () => {
    const { state } = await homeSnapshot();
    render(
      <QueryClientProvider client={browserClient()}>
        <PrefetchBoundary state={flight(state)} snapshotId={freshId()}>
          <Consumers />
        </PrefetchBoundary>
      </QueryClientProvider>,
    );
    expect(
      screen.getByText("pinned=1 continue=2 libraries=1", { exact: false }),
    ).toBeTruthy();
    await new Promise((r) => setTimeout(r, 20));
    expect(net.urls).toEqual([]);
  });

  it("an infinite rail with an undefined initial page param (New in series) hydrates and can page on", async () => {
    const { state } = await prefetchPageRails(createServerQueryClient(), {
      pageId: "nis",
      isSystem: false,
      fetch: async <T,>(path: string): Promise<T> => {
        if (path === "/me/saved-views?pinned_on=nis")
          return {
            items: [
              {
                id: "v",
                name: "New",
                kind: "system",
                system_key: "new_in_series",
              },
            ],
          } as T;
        if (path === "/me/new-in-series?limit=24")
          return { items: [{ id: "a" }, { id: "b" }], next_cursor: "c2" } as T;
        throw new Error(`404 ${path}`);
      },
    });
    const wire = flight(state);
    const q = wire.queries.find(
      (x) =>
        JSON.stringify(x.queryKey) === JSON.stringify(queryKeys.newInSeries),
    );
    // Flight keeps the undefined param; JSON would have made it null and
    // the next refetch would send `cursor=null`.
    expect((q?.state.data as { pageParams: unknown[] }).pageParams).toEqual([
      undefined,
    ]);
    function Nis() {
      const r = useNewInSeriesInfinite();
      return (
        <p>
          nis={r.data?.pages[0]?.items.length ?? "none"} next=
          {String(r.hasNextPage)}
        </p>
      );
    }
    render(
      <QueryClientProvider client={browserClient()}>
        <PrefetchBoundary state={wire} snapshotId={freshId()}>
          <Nis />
        </PrefetchBoundary>
      </QueryClientProvider>,
    );
    expect(screen.getByText("nis=2 next=true", { exact: false })).toBeTruthy();
    await new Promise((r) => setTimeout(r, 20));
    expect(net.urls).toEqual([]);
  });

  it("an infinite rail (CBL window) hydrates under the hook's key too", async () => {
    const { state } = await prefetchPageRails(createServerQueryClient(), {
      pageId: "cbl-page",
      isSystem: false,
      fetch: async <T,>(path: string): Promise<T> => {
        if (path === "/me/saved-views?pinned_on=cbl-page")
          return {
            items: [
              { id: "v", name: "List", kind: "cbl", cbl_list_id: "cbl-1" },
            ],
          } as T;
        if (path === "/me/cbl-lists/cbl-1/window-paginated?before=3&after=24")
          return { items: [{ position: 1 }, { position: 2 }] } as T;
        throw new Error(`404 ${path}`);
      },
    });
    function CblConsumer() {
      const q = useCblListWindowInfinite("cbl-1", CBL_RAIL_WINDOW);
      return <p>cbl={q.data?.pages[0]?.items.length ?? "none"}</p>;
    }
    render(
      <QueryClientProvider client={browserClient()}>
        <PrefetchBoundary state={flight(state)} snapshotId={freshId()}>
          <CblConsumer />
        </PrefetchBoundary>
      </QueryClientProvider>,
    );
    expect(screen.getByText("cbl=2")).toBeTruthy();
    await new Promise((r) => setTimeout(r, 20));
    expect(net.urls).toEqual([]);
  });

  it("hydrates on first render even when the shell created an empty entry first", async () => {
    const { state } = await homeSnapshot();
    function ShellSearch() {
      useContinueReading({ enabled: false });
      return null;
    }
    const renders: string[] = [];
    function Rail() {
      const cr = useContinueReading();
      renders.push(String(cr.data?.items.length ?? "none"));
      return <p>rail={cr.data?.items.length ?? "none"}</p>;
    }
    render(
      <QueryClientProvider client={browserClient()}>
        <ShellSearch />
        <PrefetchBoundary state={flight(state)} snapshotId={freshId()}>
          <Rail />
        </PrefetchBoundary>
      </QueryClientProvider>,
    );
    // Data on the very first render (what SSR and hydration paint), not
    // a skeleton followed by an effect-time update.
    expect(renders[0]).toBe("2");
    await new Promise((r) => setTimeout(r, 20));
    expect(net.urls).toEqual([]);
  });

  it("also when that shell observer is already subscribed (shell commits before the page)", async () => {
    const errors = vi.spyOn(console, "error").mockImplementation(() => {});
    const { state } = await homeSnapshot();
    function ShellSearch() {
      const q = useContinueReading({ enabled: false });
      return <p>shell={q.data?.items.length ?? "none"}</p>;
    }
    const renders: string[] = [];
    function Rail() {
      const cr = useContinueReading();
      renders.push(String(cr.data?.items.length ?? "none"));
      return <p>rail={cr.data?.items.length ?? "none"}</p>;
    }
    const client = browserClient();
    const { rerender } = render(
      <QueryClientProvider client={client}>
        <ShellSearch />
      </QueryClientProvider>,
    );
    rerender(
      <QueryClientProvider client={client}>
        <ShellSearch />
        <PrefetchBoundary state={flight(state)} snapshotId={freshId()}>
          <Rail />
        </PrefetchBoundary>
      </QueryClientProvider>,
    );
    expect(renders[0]).toBe("2");
    expect(await screen.findByText("shell=2")).toBeTruthy();
    expect(net.urls).toEqual([]);
    expect(errors).not.toHaveBeenCalled();
    errors.mockRestore();
  });
});

describe("snapshot freshness (back/forward replays an old RSC payload)", () => {
  function Rail() {
    const cr = useContinueReading();
    return <p>rail={cr.data?.items.length ?? "none"}</p>;
  }

  it("a first mount hydrates; a remount of the same snapshot after the threshold does not", async () => {
    vi.useFakeTimers({ toFake: ["Date"] });
    vi.setSystemTime(1_000_000);
    const { state } = await homeSnapshot();
    const id = freshId();
    const first = render(
      <QueryClientProvider client={browserClient()}>
        <PrefetchBoundary state={flight(state)} snapshotId={id}>
          <Rail />
        </PrefetchBoundary>
      </QueryClientProvider>,
    );
    expect(screen.getByText("rail=2")).toBeTruthy();
    first.unmount();

    // Back from the reader 31 s later, rail queries already collected (a
    // fresh client stands in for an emptied cache): Next replays the same
    // payload, which must not paint the old Continue reading.
    vi.setSystemTime(1_000_000 + SNAPSHOT_FRESH_MS + 1_000);
    render(
      <QueryClientProvider client={browserClient()}>
        <PrefetchBoundary state={flight(state)} snapshotId={id}>
          <Rail />
        </PrefetchBoundary>
      </QueryClientProvider>,
    );
    expect(screen.getByText("rail=none")).toBeTruthy();
    // …and the hook fetches, as it did before the prefetch existed.
    await vi.waitFor(() => expect(net.urls).toContain("/me/continue-reading"));
  });

  it("a remount within the threshold still hydrates", async () => {
    vi.useFakeTimers({ toFake: ["Date"] });
    vi.setSystemTime(5_000_000);
    const { state } = await homeSnapshot();
    const id = freshId();
    render(
      <QueryClientProvider client={browserClient()}>
        <PrefetchBoundary state={flight(state)} snapshotId={id}>
          <p>first</p>
        </PrefetchBoundary>
      </QueryClientProvider>,
    );
    vi.setSystemTime(5_000_000 + 10_000);
    render(
      <QueryClientProvider client={browserClient()}>
        <PrefetchBoundary state={flight(state)} snapshotId={id}>
          <Rail />
        </PrefetchBoundary>
      </QueryClientProvider>,
    );
    expect(screen.getByText("rail=2")).toBeTruthy();
  });
});

describe("warm cache: existing data vs the snapshot", () => {
  const card = (id: string) => ({ issue: { id } });
  function Rail() {
    const cr = useContinueReading();
    return <p>ids={cr.data?.items.map((c) => c.issue.id).join(",")}</p>;
  }

  it("cached data older than the snapshot (fetched before this navigation) is replaced", async () => {
    vi.useFakeTimers({ toFake: ["Date"] });
    vi.setSystemTime(9_000_000);
    const client = browserClient();
    client.setQueryData(queryKeys.continueReading, {
      items: [card("old")],
    } as unknown as ContinueReadingView);
    vi.setSystemTime(9_000_000 + 5_000); // the user navigates to Home
    const { state } = await homeSnapshot();
    render(
      <QueryClientProvider client={client}>
        <PrefetchBoundary state={flight(state)} snapshotId={freshId()}>
          <Rail />
        </PrefetchBoundary>
      </QueryClientProvider>,
    );
    // The stock boundary merges existing entries in an effect.
    expect(await screen.findByText("ids=i1,i2")).toBeTruthy();
  });

  it("cached data newer than the snapshot (fetched after it was first seen) is kept", async () => {
    vi.useFakeTimers({ toFake: ["Date"] });
    vi.setSystemTime(20_000_000);
    const { state } = await homeSnapshot();
    const id = freshId();
    const client = browserClient();
    // First seen now…
    const first = render(
      <QueryClientProvider client={client}>
        <PrefetchBoundary state={flight(state)} snapshotId={id}>
          <p>first</p>
        </PrefetchBoundary>
      </QueryClientProvider>,
    );
    first.unmount();
    // …then the client refetched newer data (a progress write landed)…
    vi.setSystemTime(20_000_000 + 10_000);
    client.setQueryData(queryKeys.continueReading, {
      items: [card("newer")],
    } as unknown as ContinueReadingView);
    // …and the same snapshot is replayed within the threshold. Restamped
    // to when this browser first saw it, it is older, so it loses —
    // whatever the server's clock said.
    vi.setSystemTime(20_000_000 + 12_000);
    render(
      <QueryClientProvider client={client}>
        <PrefetchBoundary state={flight(state)} snapshotId={id}>
          <Rail />
        </PrefetchBoundary>
      </QueryClientProvider>,
    );
    await new Promise((r) => setTimeout(r, 20));
    expect(screen.getByText("ids=newer")).toBeTruthy();
  });

  it("a server clock far ahead does not make the snapshot win (restamped to the browser clock)", async () => {
    vi.useFakeTimers({ toFake: ["Date"] });
    vi.setSystemTime(30_000_000);
    const { state } = await homeSnapshot();
    const ahead: DehydratedState = {
      ...state,
      queries: state.queries.map((q) => ({
        ...q,
        state: { ...q.state, dataUpdatedAt: 30_000_000 + 3_600_000 },
      })),
    };
    const id = freshId();
    const client = browserClient();
    render(
      <QueryClientProvider client={client}>
        <PrefetchBoundary state={flight(ahead)} snapshotId={id}>
          <p>seen</p>
        </PrefetchBoundary>
      </QueryClientProvider>,
    ).unmount();
    vi.setSystemTime(30_000_000 + 5_000);
    client.setQueryData(queryKeys.continueReading, {
      items: [card("newer")],
    } as unknown as ContinueReadingView);
    render(
      <QueryClientProvider client={client}>
        <PrefetchBoundary state={flight(ahead)} snapshotId={id}>
          <Rail />
        </PrefetchBoundary>
      </QueryClientProvider>,
    );
    await new Promise((r) => setTimeout(r, 20));
    expect(screen.getByText("ids=newer")).toBeTruthy();
  });
});
