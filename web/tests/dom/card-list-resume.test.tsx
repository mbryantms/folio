// @vitest-environment jsdom
/**
 * A mark cancels in-flight card-list fetches (so an old response can't
 * land on top of it) and re-runs them when it settles; and a list that
 * mounted while the mark's request was in flight shows the mark once it
 * succeeds. Real hooks, real cards, a fake server that keeps state.
 */
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { useState } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";

type Rec = { finished: boolean; page: number; percent: number };

const net = vi.hoisted(() => ({
  calls: [] as string[],
  /** The server's progress per issue (absent = no record). */
  server: new Map<string, Rec>(),
  /** List GETs to leave hanging (never answered), counted down. */
  holdLists: 0,
  /** When set, progress POSTs wait on it before committing. */
  postGate: null as Promise<void> | null,
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
    const record = (id: string) => {
      const r = net.server.get(id);
      return r ? { ...r, updated_at: "2026-11-01T10:00:00+00:00" } : null;
    };
    if (method === "POST") {
      if (net.postGate) await net.postGate;
      const body = JSON.parse(String(init?.body ?? "{}"));
      const set = (id: string, finished: boolean, page: number) =>
        net.server.set(id, { finished, page, percent: finished ? 1 : 0 });
      if (path === "/progress") {
        set(body.issue_id, !!body.finished, body.page);
        return json({
          run: 0,
          issue_id: body.issue_id,
          ...record(body.issue_id),
        });
      }
      for (const id of body.issue_ids) set(id, !!body.finished, 0);
      return json({
        updated: body.issue_ids.length,
        skipped: 0,
        forbidden: 0,
        not_found: 0,
        failed: 0,
      });
    }
    if (path.startsWith("/progress?issue_id=")) {
      const id = path.split("=")[1]!;
      const r = record(id);
      return json({ records: r ? [{ run: 0, issue_id: id, ...r }] : [] });
    }
    // A list: the server's state as of when the request arrived.
    const items = ["a", "b"].map((id) => card(id, record(id)));
    if (net.holdLists > 0) {
      net.holdLists -= 1;
      return new Promise<Response>(() => {});
    }
    return json({ items, next_cursor: null });
  },
}));
vi.mock("sonner", () => ({
  toast: Object.assign(vi.fn(), {
    success: vi.fn(),
    error: vi.fn(),
    message: vi.fn(),
    info: vi.fn(),
    loading: vi.fn(),
    dismiss: vi.fn(),
  }),
}));
vi.mock("next/navigation", () => ({
  useRouter: () => ({ push: () => {}, refresh: () => {} }),
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
import { RecentIssuesRailBody } from "@/components/saved-views/system-rails";
import {
  useBulkMarkProgress,
  useUpsertIssueProgress,
} from "@/lib/api/mutations";
import { useSeriesIssuesInfinite } from "@/lib/api/queries";

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

function SeriesList() {
  const q = useSeriesIssuesInfinite("s1");
  return (
    <section aria-label="series">
      {q.data?.pages.flatMap((p) =>
        p.items.map((issue) => <IssueCard key={issue.id} issue={issue} />),
      )}
    </section>
  );
}

function Page({ id, bulk }: { id: string; bulk?: boolean }) {
  const single = useUpsertIssueProgress();
  const many = useBulkMarkProgress();
  const [series, setSeries] = useState(false);
  return (
    <>
      <section aria-label="rail">
        <RecentIssuesRailBody itemStyle={{}} />
      </section>
      {series && <SeriesList />}
      <button
        type="button"
        onClick={() =>
          bulk
            ? many.mutate({ issue_ids: [id], finished: true })
            : single.mutate({ issue_id: id, page: 0, finished: true })
        }
      >
        Mark
      </button>
      <button type="button" onClick={() => setSeries(true)}>
        Series
      </button>
    </>
  );
}

function mount(id: string, bulk?: boolean) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <Page id={id} bulk={bulk} />
    </QueryClientProvider>,
  );
  return client;
}

const listGets = () =>
  net.calls.filter((c) => c === "GET /me/recent-issues").length;
const posts = () => net.calls.filter((c) => c.startsWith("POST ")).length;

function readCheck(region: string, id: string): Element | null {
  return within(screen.getByRole("region", { name: region }))
    .getByRole("link", { name: new RegExp(`Issue ${id}`) })
    .querySelector('[role="img"][aria-label="Read"]');
}

function links(region: string) {
  return screen.getByRole("region", { name: region }).querySelectorAll("a")
    .length;
}

beforeEach(() => {
  net.calls.length = 0;
  net.server = new Map();
  net.holdLists = 0;
  net.postGate = null;
});

describe("cancelled card-list fetches resume when the mark settles", () => {
  it("a rail whose first fetch a mark cancelled loads after the mark", async () => {
    net.holdLists = 1;
    mount("z");
    await waitFor(() => expect(listGets()).toBe(1));
    expect(links("rail")).toBe(0);
    fireEvent.click(screen.getByRole("button", { name: "Mark" }));
    await waitFor(() => expect(posts()).toBe(1));
    // The cancelled first fetch is made again, and the skeleton goes.
    await waitFor(() => expect(listGets()).toBe(2));
    await waitFor(() => expect(links("rail")).toBe(2));
  });

  it("a refetch a mark cancelled is resumed and ends with the server's newer data plus the mark", async () => {
    const client = mount("a");
    await waitFor(() => expect(links("rail")).toBe(2));
    expect(readCheck("rail", "b")).toBeNull();
    // Another device finishes b; the rail refetches (held in flight)…
    net.server.set("b", { finished: true, page: 19, percent: 1 });
    net.holdLists = 1;
    void client.refetchQueries({ queryKey: ["rails", "recent-issues"] });
    await waitFor(() => expect(listGets()).toBe(2));
    // …and the mark of a cancels it.
    fireEvent.click(screen.getByRole("button", { name: "Mark" }));
    await waitFor(() => expect(readCheck("rail", "a")).not.toBeNull());
    await waitFor(() => expect(listGets()).toBe(3));
    await waitFor(() => expect(readCheck("rail", "b")).not.toBeNull());
    expect(readCheck("rail", "a")).not.toBeNull();
  });
});

describe("a list mounted during the mark's request", () => {
  it.each([
    ["single mark", false],
    ["bulk mark", true],
  ])("shows the %s after it succeeds", async (_name, bulk) => {
    mount("a", bulk);
    await waitFor(() => expect(links("rail")).toBe(2));
    let commit!: () => void;
    net.postGate = new Promise((r) => (commit = r));
    fireEvent.click(screen.getByRole("button", { name: "Mark" }));
    await waitFor(() => expect(readCheck("rail", "a")).not.toBeNull());
    // The series list mounts and fetches before the server commits.
    fireEvent.click(screen.getByRole("button", { name: "Series" }));
    await waitFor(() => expect(links("series")).toBe(2));
    expect(readCheck("series", "a")).toBeNull();
    await act(async () => {
      commit();
    });
    await waitFor(() => expect(readCheck("series", "a")).not.toBeNull());
    expect(readCheck("rail", "a")).not.toBeNull();
    expect(readCheck("series", "b")).toBeNull();
  });
});
