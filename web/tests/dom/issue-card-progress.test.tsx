// @vitest-environment jsdom
/**
 * Issue cards draw their read check / progress bar from the card's own
 * `progress` (embedded by the server on every issue-card list), so a
 * badge-only surface makes no `GET /progress` request (every record of
 * the user). A card from a response without the field still falls back
 * to it. A progress write made in this tab (the progress overlay) moves
 * the badge without refetching the list, and is ordered against list
 * copies by arrival, not by clock.
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
import {
  applyOptimisticProgress,
  settleProgress,
} from "@/lib/api/mutations/progress";
import { progressOverlay } from "@/lib/api/progress-overlay";
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

    // The reader's write reply lands in the overlay.
    act(() => {
      progressOverlay(client).write(
        "a",
        { finished: true, page: 19, percent: 1 },
        false,
      );
    });
    await waitFor(() => expect(readCheck("a")).not.toBeNull());
    expect(net.calls).toEqual(["/me/recent-issues"]);
  });

  // The overlay orders copies by when this tab saw them, never by clock:
  // the same story must come out the same with the browser's clock a day
  // behind or a day ahead of the server's stamps.
  for (const skewDays of [-1, 1]) {
    it(`orders writes and list copies by arrival, clock ${skewDays < 0 ? "behind" : "ahead"}`, async () => {
      vi.useFakeTimers({ toFake: ["Date"] });
      vi.setSystemTime(Date.parse(T0) + skewDays * 86_400_000);
      try {
        // Finished on the server (stamped T0) and so on the list.
        net.items = [
          card("a", { finished: true, page: 19, percent: 1, updated_at: T0 }),
        ];
        const { client } = setup();
        await waitFor(() => expect(readCheck("a")).not.toBeNull());

        // Bulk-mark unread: the optimistic entry (client stamp) shows,
        // whatever the clock says, and stays while pending even when a
        // list fetch lands meanwhile (it started before the ack).
        let snap: Awaited<ReturnType<typeof applyOptimisticProgress>>;
        await act(async () => {
          snap = await applyOptimisticProgress(client, [
            { issue_id: "a", page: 0, finished: false },
          ]);
        });
        await waitFor(() => expect(readCheck("a")).toBeNull());
        await act(() =>
          client.refetchQueries({ queryKey: queryKeys.recentIssues }),
        );
        expect(readCheck("a")).toBeNull();
        expect(snap!.issues[0]![1]).toBeUndefined();

        // Acknowledged. A list fetched after that is the truth again —
        // here another device has since marked it read.
        act(() => settleProgress(client, ["a"]));
        net.items = [
          card("a", { finished: true, page: 19, percent: 1, updated_at: T0 }),
        ];
        await act(() =>
          client.refetchQueries({ queryKey: queryKeys.recentIssues }),
        );
        await waitFor(() => expect(readCheck("a")).not.toBeNull());

        // A newer write beats that list copy until the next fetch.
        act(() => {
          progressOverlay(client).write(
            "a",
            { finished: false, page: 0, percent: 0 },
            false,
          );
        });
        await waitFor(() => expect(readCheck("a")).toBeNull());
      } finally {
        vi.useRealTimers();
      }
    });
  }
});
