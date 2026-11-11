// @vitest-environment jsdom
/**
 * D18: what the home page dehydrates on the server must be what the client
 * hooks read. Prefetch with a fake `apiGet`, hand the state to the page's
 * `<PrefetchBoundary>` under the app's client defaults, mount the real
 * hooks, and assert they render the data without a single browser request
 * — including when the shell created an empty entry for the same key
 * first (the topbar search's disabled `useContinueReading`).
 */
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { PrefetchBoundary } from "@/components/PrefetchBoundary";
import {
  useCblListWindowInfinite,
  useContinueReading,
  useLibraryList,
  useSavedViews,
} from "@/lib/api/queries";
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

function CblConsumer() {
  const q = useCblListWindowInfinite("cbl-1", CBL_RAIL_WINDOW);
  return <p>cbl={q.data?.pages[0]?.items.length ?? "none"}</p>;
}

/** The app's browser defaults (components/QueryProvider.tsx). */
const browserClient = () =>
  new QueryClient({
    defaultOptions: { queries: { staleTime: 30_000, retry: false } },
  });

describe("home prefetch → HydrationBoundary", () => {
  it("client hooks start with the prefetched data and send no request", async () => {
    net.urls.length = 0;
    const state = await prefetchPageRails(createServerQueryClient(), {
      pageId: "home",
      isSystem: true,
      fetch: serverFetch,
    });
    // Through a JSON round trip, like the RSC payload.
    const wire = JSON.parse(JSON.stringify(state));
    render(
      <QueryClientProvider client={browserClient()}>
        <PrefetchBoundary state={wire}>
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

  it("an infinite rail (CBL window) hydrates under the hook's key too", async () => {
    net.urls.length = 0;
    const qc = createServerQueryClient();
    const state = await prefetchPageRails(qc, {
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
    render(
      <QueryClientProvider client={browserClient()}>
        <PrefetchBoundary state={JSON.parse(JSON.stringify(state))}>
          <CblConsumer />
        </PrefetchBoundary>
      </QueryClientProvider>,
    );
    expect(screen.getByText("cbl=2")).toBeTruthy();
    await new Promise((r) => setTimeout(r, 20));
    expect(net.urls).toEqual([]);
  });

  it("hydrates on first render even when the shell created an empty entry first", async () => {
    net.urls.length = 0;
    const state = await prefetchPageRails(createServerQueryClient(), {
      pageId: "home",
      isSystem: true,
      fetch: serverFetch,
    });
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
        <PrefetchBoundary state={JSON.parse(JSON.stringify(state))}>
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
    net.urls.length = 0;
    const errors = vi.spyOn(console, "error").mockImplementation(() => {});
    const state = await prefetchPageRails(createServerQueryClient(), {
      pageId: "home",
      isSystem: true,
      fetch: serverFetch,
    });
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
        <PrefetchBoundary state={JSON.parse(JSON.stringify(state))}>
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
