// @vitest-environment jsdom
/**
 * A failed mark and a failed Undo put mounted issue cards back where they
 * were. A mark patches the cards' embedded progress in the cached lists;
 * a rollback must write those copies back (notifying the mounted cards),
 * or a failed bulk mark from the selection toolbar leaves every card
 * showing the optimistic check.
 */
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const net = vi.hoisted(() => ({
  calls: [] as string[],
  items: [] as unknown[],
  failPosts: false,
  /** When set, list GETs wait on it. */
  gate: null as Promise<void> | null,
}));

vi.mock("@/lib/api/auth-refresh", () => ({
  getCsrfToken: () => null,
  apiFetch: async (path: string, init?: RequestInit) => {
    net.calls.push(`${init?.method ?? "GET"} ${path}`);
    if ((init?.method ?? "GET") !== "GET") {
      return net.failPosts
        ? new Response(
            JSON.stringify({ error: { code: "internal", message: "boom" } }),
            { status: 500, headers: { "Content-Type": "application/json" } },
          )
        : new Response(JSON.stringify({ updated: 1 }), {
            status: 200,
            headers: { "Content-Type": "application/json" },
          });
    }
    const items = net.items;
    if (net.gate) await net.gate;
    return new Response(JSON.stringify({ items }), {
      status: 200,
      headers: { "Content-Type": "application/json" },
    });
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

import { RecentIssuesRailBody } from "@/components/saved-views/system-rails";
import {
  applyOptimisticProgress,
  capturePriorProgress,
  restoreProgress,
  rollbackProgress,
} from "@/lib/api/mutations/progress";

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

async function mount() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <RecentIssuesRailBody itemStyle={{}} />
    </QueryClientProvider>,
  );
  await waitFor(() => expect(screen.getAllByRole("link").length).toBe(3));
  return client;
}

beforeEach(() => {
  net.calls.length = 0;
  net.failPosts = false;
  net.gate = null;
  net.items = [
    card("a", null),
    card("b", null),
    card("c", { finished: true, page: 19, percent: 1, updated_at: T0 }),
  ];
});

describe("rollback reaches mounted cards", () => {
  it("a failed bulk mark clears the optimistic checks", async () => {
    const client = await mount();
    let snap: Awaited<ReturnType<typeof applyOptimisticProgress>>;
    await act(async () => {
      snap = await applyOptimisticProgress(client, [
        { issue_id: "a", page: 0, finished: true },
        { issue_id: "b", page: 0, finished: true },
      ]);
    });
    await waitFor(() => expect(readCheck("a")).not.toBeNull());
    expect(readCheck("b")).not.toBeNull();
    // The POST failed: the mutation's onError rolls back.
    act(() => rollbackProgress(client, snap));
    await waitFor(() => expect(readCheck("a")).toBeNull());
    expect(readCheck("b")).toBeNull();
    expect(readCheck("c")).not.toBeNull();
  });

  it("a failed Undo puts the marked state back", async () => {
    const client = await mount();
    // Mark a + b read (succeeded), then Undo them to "no record" — and
    // the Undo's POST fails, so the server still has them read.
    await act(async () => {
      await applyOptimisticProgress(client, [
        { issue_id: "a", page: 0, finished: true },
        { issue_id: "b", page: 0, finished: true },
      ]);
    });
    await waitFor(() => expect(readCheck("a")).not.toBeNull());
    // The server has them read now.
    net.items = [
      card("a", { finished: true, page: 0, percent: 1, updated_at: T0 }),
      card("b", { finished: true, page: 0, percent: 1, updated_at: T0 }),
      net.items[2],
    ];
    net.failPosts = true;
    // Hold the list refetch the failure triggers, so the rollback itself
    // is what's on screen.
    let open!: () => void;
    net.gate = new Promise((r) => (open = r));
    await act(async () => {
      await restoreProgress(
        client,
        new Map([
          ["a", null],
          ["b", null],
        ]),
      ).catch(() => {});
    });
    // Undo's own optimistic step cleared the checks; its failure must
    // bring them back before the refetch lands…
    await waitFor(() => expect(readCheck("a")).not.toBeNull());
    expect(readCheck("b")).not.toBeNull();
    // …and the refetch (the server's state) keeps them.
    await act(async () => {
      open();
      await new Promise((r) => setTimeout(r, 50));
    });
    expect(readCheck("a")).not.toBeNull();
    expect(readCheck("b")).not.toBeNull();
  });

  it("a bulk mark of cards on screen asks the network for no priors, and Undo restores each", async () => {
    net.items = [
      card("a", null),
      card("b", { finished: false, page: 5, percent: 0.25, updated_at: T0 }),
      card("c", { finished: true, page: 19, percent: 1, updated_at: T0 }),
    ];
    const client = await mount();
    const ids = ["a", "b", "c"];
    let prior: Awaited<ReturnType<typeof capturePriorProgress>>;
    await act(async () => {
      // `beginMark`: priors first, then the optimistic write; then the
      // server acknowledges.
      const priorP = capturePriorProgress(client, ids);
      await applyOptimisticProgress(
        client,
        ids.map((issue_id) => ({ issue_id, page: 0, finished: true })),
      );
      prior = await priorP;
    });
    expect(prior!.get("a")).toBeNull();
    expect(prior!.get("b")).toMatchObject({ page: 5, percent: 0.25 });
    expect(prior!.get("c")).toMatchObject({ finished: true });
    await waitFor(() => expect(readCheck("a")).not.toBeNull());
    expect(readCheck("b")).not.toBeNull();

    await act(() => restoreProgress(client, prior!));
    await waitFor(() => expect(readCheck("a")).toBeNull());
    expect(readCheck("b")).toBeNull();
    expect(
      screen
        .getByRole("link", { name: /Issue b/ })
        .querySelector<HTMLElement>('[style*="width"]')?.style.width,
    ).toBe("25%");
    expect(readCheck("c")).not.toBeNull();
    // The list and the Undo's writes — never `GET /progress`.
    expect(net.calls.filter((c) => c.startsWith("GET /progress"))).toEqual([]);
    expect(net.calls).toContain("POST /progress");
  });
});
