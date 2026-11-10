// @vitest-environment jsdom
/**
 * Progress sync while the browser stays online but the server does not
 * answer (a restart, a proxy error): the outbox replayer is the one retry
 * loop. A failed write is retried by its backoff timer or by the server
 * becoming reachable — no page turn, no visibility change — the backoff
 * never spins against a server that stays down, one notice says when a
 * stuck queue drained, and the connectivity toasts say what is kept.
 */
import { act, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const api = vi.hoisted(() => ({ send: vi.fn() }));
const toasts = vi.hoisted(() => ({
  success: vi.fn(),
  warning: vi.fn(),
  message: vi.fn(),
  dismiss: vi.fn(),
}));
vi.mock("@/lib/api/auth-refresh", () => ({
  apiFetch: api.send,
  getCsrfToken: () => "csrf",
}));
vi.mock("sonner", () => ({ toast: toasts }));
vi.mock("@/components/AccountCacheBoundary", () => ({
  AccountCacheBoundary: () => null,
}));
vi.mock("@/components/offline/OfflineBootstrap", () => ({
  OfflineBootstrap: () => null,
}));

import { TOAST } from "@/lib/api/toast-strings";
import { CONNECTIVITY_EVENT } from "@/lib/pwa/connectivity";
import {
  createOutbox,
  getOutbox,
  memoryStorage,
  startOutboxReplay,
  type Outbox,
} from "@/lib/pwa/outbox";
import {
  createProgressWriter,
  PROGRESS_OUTBOX_KIND,
  progressOutboxKind,
  type ProgressBody,
} from "@/lib/reader/progress-writer";
import { OutboxReplayer } from "@/components/OutboxReplayer";
import { QueryProvider } from "@/components/QueryProvider";
import {
  progressSyncState,
  ProgressSyncStatus,
} from "@/app/[locale]/read/[seriesSlug]/[issueSlug]/ProgressSyncStatus";

const refused = () => Promise.reject(new TypeError("Failed to fetch"));
const ok = () => Promise.resolve(new Response("{}", { status: 200 }));

/** An outbox with the progress kind registered against `post`. */
function setup() {
  const post = vi.fn<(body: ProgressBody) => Promise<Response>>(refused);
  const outbox = createOutbox({ storage: memoryStorage() });
  outbox.register(PROGRESS_OUTBOX_KIND, progressOutboxKind(post));
  return { post, outbox };
}

/** The live reader path: a writer mirroring into `outbox`, sending via
 *  `post`. */
function liveWriter(
  outbox: Outbox,
  post: (body: ProgressBody) => Promise<Response>,
) {
  return createProgressWriter(async (body) => (await post(body)).ok, {
    outbox,
  });
}

const queued = async (outbox: Outbox) =>
  (await outbox.entries({ kind: PROGRESS_OUTBOX_KIND })).length;

const settle = () => vi.advanceTimersByTimeAsync(0);

let stops: Array<() => void> = [];
beforeEach(() => {
  vi.useFakeTimers();
});
afterEach(() => {
  for (const stop of stops) stop();
  stops = [];
  vi.useRealTimers();
  vi.clearAllMocks();
  api.send.mockReset();
});

describe("retry while online", () => {
  it("replays a failed write on the backoff timer, without a page turn", async () => {
    const { post, outbox } = setup();
    stops.push(startOutboxReplay(outbox));
    await settle();

    const writer = liveWriter(outbox, post);
    writer.set({ issue_id: "a", page: 4 });
    await writer.flush(); // server refuses
    await settle();
    expect(post).toHaveBeenCalledTimes(1);
    expect(await queued(outbox)).toBe(1);

    // The server is back; the reader just sits on the page.
    post.mockImplementation(ok);
    await vi.advanceTimersByTimeAsync(5_000);
    expect(post).toHaveBeenCalledTimes(2);
    expect(post.mock.calls[1]![0]).toEqual({ issue_id: "a", page: 4 });
    expect(await queued(outbox)).toBe(0);
  });

  it("replays at once when the server reports reachable again", async () => {
    const { post, outbox } = setup();
    stops.push(startOutboxReplay(outbox));
    await settle();
    const writer = liveWriter(outbox, post);
    writer.set({ issue_id: "a", page: 2 });
    await writer.flush();
    await settle();

    post.mockImplementation(ok);
    window.dispatchEvent(
      new CustomEvent(CONNECTIVITY_EVENT, { detail: "reachable" }),
    );
    await settle();
    expect(post).toHaveBeenCalledTimes(2);
    expect(await queued(outbox)).toBe(0);
    // The armed backoff timer was consumed by that pass: nothing more.
    await vi.advanceTimersByTimeAsync(600_000);
    expect(post).toHaveBeenCalledTimes(2);
  });

  it("backs off 5 s → 5 min against a server that stays down, one timer", async () => {
    const { post, outbox } = setup();
    await outbox.enqueue(PROGRESS_OUTBOX_KIND, "a:current", {
      issue_id: "a",
      page: 1,
    });
    stops.push(startOutboxReplay(outbox));
    await settle();
    expect(post).toHaveBeenCalledTimes(1);

    const at = (ms: number) => vi.advanceTimersByTimeAsync(ms);
    const steps = [5_000, 15_000, 30_000, 60_000, 120_000, 300_000, 300_000];
    for (const [i, delay] of steps.entries()) {
      await at(delay - 1);
      expect(post).toHaveBeenCalledTimes(i + 1);
      // New writes while the loop waits do not arm a second timer.
      await outbox.enqueue(PROGRESS_OUTBOX_KIND, "a:current", {
        issue_id: "a",
        page: 2 + i,
      });
      expect(vi.getTimerCount()).toBe(1);
      await at(1);
      expect(post).toHaveBeenCalledTimes(i + 2);
    }
  });

  it("stops when the outbox is empty and when unmounted", async () => {
    const { post, outbox } = setup();
    post.mockImplementation(ok);
    const stop = startOutboxReplay(outbox);
    await settle();
    expect(vi.getTimerCount()).toBe(0);

    await outbox.enqueue(PROGRESS_OUTBOX_KIND, "a:current", {
      issue_id: "a",
      page: 1,
    });
    expect(vi.getTimerCount()).toBe(1);
    stop();
    expect(vi.getTimerCount()).toBe(0);
    await vi.advanceTimersByTimeAsync(600_000);
    expect(post).not.toHaveBeenCalled();
  });
});

describe("drain notice", () => {
  it("fires once after a stuck queue empties, not after a normal write", async () => {
    const { post, outbox } = setup();
    const onDrained = vi.fn();
    stops.push(startOutboxReplay(outbox, { onDrained }));
    await settle();

    // An ordinary write: delivered live, the armed pass finds nothing.
    post.mockImplementation(ok);
    const writer = liveWriter(outbox, post);
    writer.set({ issue_id: "a", page: 1 });
    await writer.flush();
    await vi.advanceTimersByTimeAsync(5_000);
    expect(await queued(outbox)).toBe(0);
    expect(onDrained).not.toHaveBeenCalled();

    // Stuck: the write and the first replay both fail.
    post.mockImplementation(refused);
    writer.set({ issue_id: "a", page: 2 });
    await writer.flush();
    await vi.advanceTimersByTimeAsync(5_000);
    expect(await queued(outbox)).toBe(1);
    expect(onDrained).not.toHaveBeenCalled();

    // Back up: the next backoff pass drains it — one notice.
    post.mockImplementation(ok);
    await vi.advanceTimersByTimeAsync(15_000);
    expect(await queued(outbox)).toBe(0);
    expect(onDrained).toHaveBeenCalledTimes(1);

    // Later ordinary writes stay quiet.
    writer.set({ issue_id: "a", page: 3 });
    await writer.flush();
    await vi.advanceTimersByTimeAsync(600_000);
    expect(onDrained).toHaveBeenCalledTimes(1);
  });

  it("OutboxReplayer shows one success toast when the queue drains", async () => {
    api.send.mockImplementation(refused);
    await getOutbox().clear();
    await getOutbox().enqueue(PROGRESS_OUTBOX_KIND, "a:current", {
      issue_id: "a",
      page: 5,
    });
    const view = render(
      <QueryClientProvider client={new QueryClient()}>
        <OutboxReplayer userId="u1" />
      </QueryClientProvider>,
    );
    await act(() => settle());
    expect(api.send).toHaveBeenCalledTimes(1);
    expect(toasts.success).not.toHaveBeenCalled();

    api.send.mockImplementation(ok);
    await act(() => vi.advanceTimersByTimeAsync(5_000));
    expect(api.send).toHaveBeenCalledTimes(2);
    expect(toasts.success).toHaveBeenCalledTimes(1);
    expect(toasts.success).toHaveBeenCalledWith(
      TOAST.PROGRESS_SYNCED,
      expect.objectContaining({ id: "network-status" }),
    );
    view.unmount();
  });
});

describe("connectivity toast copy", () => {
  const wrap = ({ children }: { children: ReactNode }) => (
    <QueryProvider>{children}</QueryProvider>
  );

  it("says progress is kept when the server is unreachable or offline", () => {
    render(<div />, { wrapper: wrap });
    act(() => {
      window.dispatchEvent(
        new CustomEvent(CONNECTIVITY_EVENT, { detail: "unreachable" }),
      );
    });
    expect(toasts.warning).toHaveBeenLastCalledWith(
      "Folio can’t reach your server. Reading progress is kept on this device and will sync when it’s back. Other changes may not save.",
      expect.objectContaining({ id: "network-status" }),
    );
    act(() => {
      window.dispatchEvent(
        new CustomEvent(CONNECTIVITY_EVENT, { detail: "offline" }),
      );
    });
    expect(toasts.warning).toHaveBeenLastCalledWith(
      "You’re offline. Reading progress is kept on this device and will sync when you reconnect. Other changes may not save.",
      expect.objectContaining({ id: "network-status" }),
    );
    act(() => {
      window.dispatchEvent(new Event("offline"));
    });
    expect(toasts.warning).toHaveBeenLastCalledWith(
      TOAST.OFFLINE,
      expect.anything(),
    );
    for (const [message] of toasts.warning.mock.calls) {
      expect(message).not.toMatch(/cannot be saved/i);
    }
  });
});

describe("reader status line", () => {
  it("maps the outbox onto saved / queued / retrying for this issue", async () => {
    const outbox = createOutbox({ storage: memoryStorage() });
    expect(await progressSyncState(outbox, "a", true)).toBe("saved");
    await outbox.enqueue(PROGRESS_OUTBOX_KIND, "b:current", {
      issue_id: "b",
      page: 1,
    });
    expect(await progressSyncState(outbox, "a", true)).toBe("saved");
    await outbox.acknowledge(PROGRESS_OUTBOX_KIND, "b:current", {
      issue_id: "b",
      page: 1,
    });
    await outbox.enqueue(PROGRESS_OUTBOX_KIND, "a:current", {
      issue_id: "a",
      page: 1,
    });
    expect(await progressSyncState(outbox, "a", true)).toBe("queued");
    outbox.register(PROGRESS_OUTBOX_KIND, progressOutboxKind(refused));
    await outbox.replay();
    expect(await progressSyncState(outbox, "a", true)).toBe("retrying");
    expect(await progressSyncState(outbox, "a", false)).toBe("queued");
  });

  it("renders the line and follows the queue", async () => {
    vi.useRealTimers();
    await getOutbox().clear();
    render(<ProgressSyncStatus issueId="a" />);
    await waitFor(() =>
      expect(screen.getByRole("status").textContent).toBe("Progress saved"),
    );
    await act(() =>
      getOutbox().enqueue(PROGRESS_OUTBOX_KIND, "a:current", {
        issue_id: "a",
        page: 3,
      }),
    );
    await waitFor(() =>
      expect(screen.getByRole("status").textContent).toBe(
        "Progress saved on this device, will sync",
      ),
    );
    await act(() => getOutbox().clear());
    await waitFor(() =>
      expect(screen.getByRole("status").textContent).toBe("Progress saved"),
    );
  });
});
