// @vitest-environment jsdom
/**
 * Issue cards draw their read check / progress bar from the card's own
 * `progress` (embedded by the server on every issue-card list), so a
 * badge-only surface makes no `GET /progress` request (every record of
 * the user). A card from a response without the field still falls back
 * to it. A progress write made in this tab (`queryKeys.issueProgress`)
 * moves the badge without refetching the list.
 */
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const net = vi.hoisted(() => ({
  calls: [] as string[],
  items: [] as unknown[],
  records: [] as unknown[],
}));

vi.mock("@/lib/api/auth-refresh", () => ({
  getCsrfToken: () => null,
  apiFetch: async (path: string) => {
    net.calls.push(path);
    const body = path.startsWith("/progress")
      ? { records: net.records }
      : { items: net.items };
    return new Response(JSON.stringify(body), {
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
vi.mock("@/lib/api/mutations", () => ({
  useUpsertIssueProgress: () => ({ mutate: () => {}, isPending: false }),
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
import { queryKeys } from "@/lib/api/query-keys";

const T0 = "2026-11-01T10:00:00+00:00";

function card(id: string, progress?: unknown) {
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
    ...(progress === undefined ? {} : { progress }),
  };
}

function setup() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const view = render(
    <QueryClientProvider client={client}>
      <RecentIssuesRailBody itemStyle={{}} />
    </QueryClientProvider>,
  );
  return { client, ...view };
}

/** The card link for issue `id` (its accessible name starts with the title). */
function cardFor(id: string): HTMLElement {
  return screen.getByRole("link", { name: new RegExp(`Issue ${id}`) });
}

function readCheck(id: string): Element | null {
  return cardFor(id).querySelector('[role="img"][aria-label="Read"]');
}

function bar(id: string): HTMLElement | null {
  return cardFor(id).querySelector<HTMLElement>('[style*="width"]');
}

beforeEach(() => {
  net.calls.length = 0;
  net.items = [];
  net.records = [];
});

describe("issue card progress", () => {
  it("draws badges from the embedded progress without requesting /progress", async () => {
    net.items = [
      card("a", { finished: true, page: 19, percent: 1, updated_at: T0 }),
      card("b", { finished: false, page: 5, percent: 0.25, updated_at: T0 }),
      card("c", null),
    ];
    setup();
    await waitFor(() => expect(cardFor("a")).toBeTruthy());
    expect(readCheck("a")).not.toBeNull();
    expect(readCheck("b")).toBeNull();
    expect(bar("b")?.style.width).toBe("25%");
    expect(readCheck("c")).toBeNull();
    expect(bar("c")).toBeNull();
    // Let any query a card might have started settle before counting.
    await act(async () => {});
    expect(net.calls).toEqual(["/me/recent-issues"]);
  });

  it("falls back to /progress for a card from a response without the field", async () => {
    net.items = [card("a")];
    net.records = [
      {
        issue_id: "a",
        page: 19,
        percent: 1,
        finished: true,
        run: 0,
        updated_at: T0,
        finished_at: T0,
      },
    ];
    setup();
    await waitFor(() => expect(readCheck("a")).not.toBeNull());
    expect(net.calls).toContain("/progress");
  });

  it("follows a progress write made in this tab without refetching the list", async () => {
    net.items = [card("a", null)];
    const { client } = setup();
    await waitFor(() => expect(cardFor("a")).toBeTruthy());
    expect(readCheck("a")).toBeNull();

    // The reader's write reply / an optimistic mark lands here.
    act(() => {
      client.setQueryData(queryKeys.issueProgress("a"), {
        issue_id: "a",
        page: 19,
        percent: 1,
        finished: true,
        run: 0,
        updated_at: "2026-11-01T11:00:00+00:00",
      });
    });
    await waitFor(() => expect(readCheck("a")).not.toBeNull());

    // An older write never overrides the list's newer copy.
    act(() => {
      client.setQueryData(queryKeys.issueProgress("a"), {
        issue_id: "a",
        page: 0,
        percent: 0,
        finished: false,
        run: 0,
        updated_at: "2026-11-01T09:00:00+00:00",
      });
    });
    await waitFor(() => expect(readCheck("a")).toBeNull());
    expect(net.calls).toEqual(["/me/recent-issues"]);
  });
});
