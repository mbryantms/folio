// @vitest-environment jsdom
/**
 * WP-4.5 durable outbox: progress and reading-session writes that never
 * got out (tab killed offline) replay on the next launch, and replays are
 * safe under the reading-run model — they never move progress backwards,
 * never open a second run, and never resurrect a run the server has left.
 */
import "fake-indexeddb/auto";
import { act, render, renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const api = vi.hoisted(() => ({ send: vi.fn() }));
vi.mock("@/lib/api/auth-refresh", () => ({
  apiFetch: api.send,
  getCsrfToken: () => "csrf",
}));
vi.mock("@/lib/api/mutations", () => ({ invalidateRails: vi.fn() }));
vi.mock("next/navigation", () => ({ usePathname: () => "/read/s/a" }));

import {
  createOutbox,
  indexedDbStorage,
  type OutboxEntry,
} from "@/lib/pwa/outbox";
import {
  createProgressWriter,
  PROGRESS_OUTBOX_KIND,
  progressOutboxKind,
  type ProgressBody,
} from "@/lib/reader/progress-writer";
import { useReaderProgressWrite } from "@/lib/reader/use-progress-write";
import { useReadingSession } from "@/lib/reader/session";

const offline = () => Promise.reject(new TypeError("Failed to fetch"));
const ok = (body: unknown = {}) =>
  new Response(JSON.stringify(body), { status: 200 });
const client = new QueryClient();
const wrapper = ({ children }: { children: ReactNode }) => (
  <QueryClientProvider client={client}>{children}</QueryClientProvider>
);
/** A fresh handle on the app's IndexedDB queue — what a relaunch sees. */
const appQueue = () => createOutbox({ storage: indexedDbStorage() });
let dbCounter = 0;
/** An isolated durable queue; call again with the same name to "relaunch". */
const namedQueue = (name: string) =>
  createOutbox({ storage: indexedDbStorage(name) });

/**
 * The server's run rule (`api/progress.rs::upsert_for_run`, WP-1.3 +
 * WP-4.5 idempotent restart), so the client sequencing can be checked
 * end to end. The Rust integration tests are the source of truth.
 */
function fakeServer(initial?: {
  run: number;
  page: number;
  finished?: boolean;
}) {
  let rec = initial ? { finished: false, ...initial } : null;
  return {
    get record() {
      return rec;
    },
    apply(body: ProgressBody) {
      if (!rec) {
        rec = {
          run: body.restart && body.run !== undefined ? body.run + 1 : 0,
          page: body.page,
          finished: body.finished ?? false,
        };
        return rec;
      }
      let restart = !!body.restart;
      let run = body.run;
      if (restart && run !== undefined && rec.run >= run + 1) {
        restart = false;
        if (rec.run === run + 1) run = rec.run;
      }
      if (!restart && run !== undefined && run < rec.run) return rec;
      rec = restart
        ? {
            run: rec.run + 1,
            page: body.page,
            finished: body.finished ?? false,
          }
        : {
            run: rec.run,
            page:
              body.finished === undefined
                ? Math.max(rec.page, body.page)
                : body.page,
            finished: body.finished ?? rec.finished,
          };
      return rec;
    },
  };
}

beforeEach(async () => {
  localStorage.setItem("folio:account-id", "u1");
  await appQueue().clear();
});
afterEach(() => {
  vi.useRealTimers();
  vi.clearAllMocks();
  api.send.mockReset();
});

describe("kill and relaunch", () => {
  it("replays a progress write the killed tab never delivered", async () => {
    api.send.mockImplementation(offline);
    const { rerender, unmount } = renderHook(
      ({ page }: { page: number }) =>
        useReaderProgressWrite({
          issueId: "a",
          currentPage: page,
          initialPage: 0,
          initialRun: 1,
          totalPages: 20,
          incognito: false,
        }),
      { wrapper, initialProps: { page: 3 } },
    );
    rerender({ page: 7 });
    await waitFor(async () => {
      const [entry] = await appQueue().entries<ProgressBody>();
      expect(entry?.payload).toEqual({ issue_id: "a", page: 7, run: 1 });
    });
    // The tab dies: the unmount flush fails too, nothing reached the server.
    unmount();
    await waitFor(() => expect(api.send).toHaveBeenCalled());
    expect(await appQueue().entries()).toHaveLength(1);

    // Relaunch: fresh module graph (new in-memory state), same IndexedDB.
    vi.resetModules();
    api.send.mockReset().mockResolvedValue(ok({ run: 1 }));
    const { OutboxReplayer } = await import("@/components/OutboxReplayer");
    const view = render(<OutboxReplayer userId="u1" />, { wrapper });
    await waitFor(() => expect(api.send).toHaveBeenCalledTimes(1));
    const [path, init] = api.send.mock.calls[0]!;
    expect(path).toBe("/progress");
    expect(init.headers["X-CSRF-Token"]).toBe("csrf");
    expect(JSON.parse(init.body)).toEqual({ issue_id: "a", page: 7, run: 1 });
    await waitFor(async () =>
      expect(await appQueue().entries()).toHaveLength(0),
    );
    view.unmount();
  });

  it("replays a reading-session heartbeat through the same outbox", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    api.send.mockImplementation(offline);
    const { rerender, unmount } = renderHook(
      ({ page }: { page: number }) =>
        useReadingSession({
          issueId: "a",
          totalPages: 20,
          currentPage: page,
          viewMode: "single",
          trackingEnabled: true,
        }),
      { wrapper, initialProps: { page: 0 } },
    );
    rerender({ page: 1 });
    await act(async () => {
      vi.advanceTimersByTime(30_000);
    });
    // waitFor polls with setInterval; hand it back the real clock.
    vi.useRealTimers();
    let queued: OutboxEntry<{ client_session_id: string }>[] = [];
    await waitFor(async () => {
      queued = await appQueue().entries({ kind: "reading-session" });
      expect(queued).toHaveLength(1);
    });
    // Kill before the final flush can run.
    api.send.mockReset();
    vi.resetModules();
    api.send.mockResolvedValue(new Response(null, { status: 201 }));
    const { OutboxReplayer } = await import("@/components/OutboxReplayer");
    render(<OutboxReplayer userId="u1" />, { wrapper });
    await waitFor(() =>
      expect(
        api.send.mock.calls.some(([path]) => path === "/me/reading-sessions"),
      ).toBe(true),
    );
    const call = api.send.mock.calls.find(
      ([path]) => path === "/me/reading-sessions",
    )!;
    expect(JSON.parse(call[1].body).client_session_id).toBe(
      queued[0]!.payload.client_session_id,
    );
    unmount();
  });
});

describe("run safety on replay", () => {
  it("coalesces per (issue, run), keeping the furthest page", async () => {
    const outbox = namedQueue(`coalesce-${dbCounter++}`);
    const writer = createProgressWriter(offline, { outbox });
    writer.seedRun(0, false);
    for (const page of [3, 9, 5]) writer.set({ issue_id: "a", page });
    await writer.flush();
    const entries = await outbox.entries<ProgressBody>();
    expect(entries.map((e) => e.payload)).toEqual([
      { issue_id: "a", page: 9, run: 0 },
    ]);
  });

  it("delivers a finished read before the offline re-read, then opens one run", async () => {
    const name = `reread-${dbCounter++}`;
    const before = namedQueue(name);
    // Finish the issue offline…
    const first = createProgressWriter(offline, { outbox: before });
    first.seedRun(0, false);
    first.set({ issue_id: "a", page: 19, finished: true });
    // …then reopen it from the cover (restart) and read a few pages.
    const reread = createProgressWriter(offline, { outbox: before });
    reread.seedRun(0, true);
    for (const page of [0, 1, 2]) reread.set({ issue_id: "a", page });
    await Promise.all([first.flush(), reread.flush()]);

    const server = fakeServer({ run: 0, page: 5 });
    const delivered: ProgressBody[] = [];
    let loseFirstReply = true;
    const after = namedQueue(name);
    after.setAccount("u1");
    after.register(
      PROGRESS_OUTBOX_KIND,
      progressOutboxKind(async (body) => {
        delivered.push(body);
        server.apply(body);
        // The restart lands but its reply is lost: the entry stays queued
        // and is delivered again on the next replay.
        if (body.restart && loseFirstReply) {
          loseFirstReply = false;
          throw new TypeError("connection reset");
        }
        return ok(server.record);
      }),
    );
    const firstPass = await after.replay();
    expect(firstPass.retained).toBe(1);
    await after.replay();

    expect(delivered.map((b) => [b.page, b.run, !!b.restart])).toEqual([
      [19, 0, false],
      [2, 0, true],
      [2, 0, true],
    ]);
    // Finished was recorded on run 0, the re-read opened run 1 exactly once.
    expect(server.record).toEqual({ run: 1, page: 2, finished: false });
    expect(await after.entries()).toHaveLength(0);
  });

  it("drops queued writes from a run the server has already left", async () => {
    const outbox = namedQueue(`stale-${dbCounter++}`);
    const writer = createProgressWriter(offline, { outbox });
    writer.seedRun(1, false);
    writer.set({ issue_id: "a", page: 15 });
    const restart = createProgressWriter(offline, { outbox });
    restart.seedRun(1, true);
    restart.set({ issue_id: "a", page: 2 });
    await Promise.all([writer.flush(), restart.flush()]);

    // Meanwhile another device re-read twice: the server is on run 3.
    const server = fakeServer({ run: 3, page: 1 });
    const post = vi.fn(async (body: ProgressBody) => ok(server.apply(body)));
    outbox.register(PROGRESS_OUTBOX_KIND, progressOutboxKind(post));
    const report = await outbox.replay();
    expect(report).toMatchObject({ delivered: 2, retained: 0 });
    expect(server.record).toEqual({ run: 3, page: 1, finished: false });
    expect(await outbox.entries()).toHaveLength(0);
    // Not retried: the server answered, the writes were just stale.
    await outbox.replay();
    expect(post).toHaveBeenCalledTimes(2);
  });

  it("never moves progress backwards when an old page replays late", async () => {
    const server = fakeServer({ run: 0, page: 12 });
    const outbox = namedQueue(`late-${dbCounter++}`);
    outbox.register(
      PROGRESS_OUTBOX_KIND,
      progressOutboxKind(async (body) => ok(server.apply(body))),
    );
    await outbox.enqueue(PROGRESS_OUTBOX_KIND, "a:0", {
      issue_id: "a",
      page: 4,
      run: 0,
    });
    await outbox.replay();
    expect(server.record?.page).toBe(12);
  });
});

describe("outbox mechanics", () => {
  it("never delivers another account's queued writes", async () => {
    const name = `accounts-${dbCounter++}`;
    const first = namedQueue(name);
    first.setAccount("u1");
    await first.enqueue("progress", "a:0", { issue_id: "a", page: 3, run: 0 });

    const other = namedQueue(name);
    other.setAccount("u2");
    const deliver = vi.fn().mockResolvedValue("done");
    other.register("progress", { deliver });
    await other.replay();
    expect(deliver).not.toHaveBeenCalled();
    expect(await other.entries()).toHaveLength(0);
  });

  it("keeps a payload merged in while its delivery was in flight", async () => {
    const outbox = namedQueue(`inflight-${dbCounter++}`);
    const seen: number[] = [];
    let release!: () => void;
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    outbox.register<ProgressBody>("progress", {
      merge: (a, b) => ({ ...b, page: Math.max(a.page, b.page) }),
      async deliver(body) {
        seen.push(body.page);
        if (seen.length === 1) await gate;
        return "done";
      },
    });
    await outbox.enqueue("progress", "a:0", { issue_id: "a", page: 4 });
    const replay = outbox.replay();
    await waitFor(() => expect(seen).toEqual([4]));
    await outbox.enqueue("progress", "a:0", { issue_id: "a", page: 6 });
    release();
    await replay;
    expect(seen).toEqual([4, 6]);
    expect(await outbox.entries()).toHaveLength(0);
  });

  it("drops permanent rejections and retains retryable ones", async () => {
    const outbox = namedQueue(`outcomes-${dbCounter++}`);
    const status = new Map([
      ["gone", 404],
      ["busy", 503],
    ]);
    outbox.register(
      PROGRESS_OUTBOX_KIND,
      progressOutboxKind(
        async (body) =>
          new Response(null, { status: status.get(body.issue_id) ?? 200 }),
      ),
    );
    await outbox.enqueue(PROGRESS_OUTBOX_KIND, "gone:0", {
      issue_id: "gone",
      page: 1,
      run: 0,
    });
    await outbox.enqueue(PROGRESS_OUTBOX_KIND, "busy:0", {
      issue_id: "busy",
      page: 1,
      run: 0,
    });
    const report = await outbox.replay();
    expect(report).toMatchObject({ dropped: 1, retained: 1 });
    const [left] = await outbox.entries();
    expect(left).toMatchObject({ key: "busy:0", attempts: 1 });
  });

  it("creates its store when the database already exists without it", async () => {
    const name = `probe-${dbCounter++}`;
    await new Promise<void>((resolve) => {
      const req = indexedDB.open(name);
      req.onsuccess = () => {
        req.result.close();
        resolve();
      };
    });
    const outbox = namedQueue(name);
    await outbox.enqueue("progress", "a:0", { issue_id: "a", page: 1 });
    expect(await namedQueue(name).entries()).toHaveLength(1);
  });
});
