import type { DeliveryOutcome, Outbox, OutboxKind } from "@/lib/pwa/outbox";
import { outcomeForStatus } from "@/lib/pwa/outbox";

export type ProgressBody = {
  issue_id: string;
  page: number;
  finished?: boolean;
  /** Reading run this write belongs to. On a `restart` write it is the
   *  run being left: the server opens `run + 1`, at most once. */
  run?: number;
  /** Open a new reading run at `page` ("Read from beginning", or a
   *  finished issue reopened from the cover). */
  restart?: boolean;
};
/** What `send` resolves to: `false` when the server rejected the write,
 * `true` or `{ run }` when it accepted it (the reply's reading run, when
 * the caller could read it). */
export type ProgressSendResult = boolean | { run?: number };

/** Outbox kind for reader progress writes (WP-4.5). */
export const PROGRESS_OUTBOX_KIND = "progress";

/** The run a write lands in: a restart targets the run after the one it
 *  leaves. `null` for an untagged write (current run). */
export function progressTargetRun(body: ProgressBody): number | null {
  if (typeof body.run !== "number") return null;
  return body.restart ? body.run + 1 : body.run;
}

/** Coalescing key: one queued write per `(issue, run)`. Different runs of
 *  one issue stay separate so a finished first read is delivered before
 *  the re-read that follows it. */
export function progressOutboxKey(body: ProgressBody): string {
  return `${body.issue_id}:${progressTargetRun(body) ?? "current"}`;
}

/**
 * Fold two writes for the same `(issue, run)`: the furthest page wins
 * (the server keeps `max(last_page)` within a run anyway), `finished` is
 * sticky, and a pending restart keeps its restart tag so the run is
 * still opened — idempotently — when the merged write is delivered.
 */
export function mergeProgress(
  stored: ProgressBody,
  incoming: ProgressBody,
): ProgressBody {
  const restart = stored.restart ? stored : incoming.restart ? incoming : null;
  const merged: ProgressBody = {
    issue_id: incoming.issue_id,
    page: Math.max(stored.page, incoming.page),
  };
  const finished =
    stored.finished === true || incoming.finished === true
      ? true
      : (incoming.finished ?? stored.finished);
  if (finished !== undefined) merged.finished = finished;
  const run = restart ? restart.run : (incoming.run ?? stored.run);
  if (run !== undefined) merged.run = run;
  if (restart) merged.restart = true;
  return merged;
}

/**
 * The outbox kind that replays queued progress writes. `post` performs the
 * request (the app passes `apiFetch`, which carries the CSRF header and
 * refreshes an expired access token). Replays are safe by construction:
 * every queued write carries its run, the server ignores older runs, keeps
 * the furthest page within a run, and applies a run-tagged restart once.
 */
export function progressOutboxKind(
  post: (body: ProgressBody) => Promise<Response>,
  onDelivered?: (body: ProgressBody) => void,
): OutboxKind<ProgressBody> {
  return {
    merge: mergeProgress,
    async deliver(body): Promise<DeliveryOutcome> {
      const response = await post(body);
      const outcome = outcomeForStatus(response.status);
      if (outcome === "done") onDelivered?.(body);
      return outcome;
    },
  };
}

/** An in-memory latest-write buffer with one pending value per
 * `(issue, run)`. Serial delivery prevents an older page acknowledgement
 * from clearing or overwriting a newer page.
 *
 * Reading runs (WP-1.3): the writer owns the run it is writing into and
 * whether the next delivered write must open a new one (`seedRun`). Bodies
 * are tagged when they are set — `restart: true` plus the run being left
 * until a restart has been acknowledged, `run` otherwise — and the run is
 * adopted from each reply, so a device follows a newer run started
 * elsewhere without re-tagging writes it queued on the old one.
 *
 * Durability (WP-4.5): with an `outbox`, every set is mirrored into the
 * IndexedDB queue and removed once delivered, so a write that never got
 * out (tab killed offline) is replayed on the next launch. */
export function createProgressWriter(
  send: (body: ProgressBody) => Promise<ProgressSendResult>,
  opts: { outbox?: Outbox } = {},
) {
  const { outbox } = opts;
  const pending = new Map<string, ProgressBody>();
  let running: Promise<void> | undefined;
  let epoch = 0;
  let run = 0;
  let restartPending = false;
  let seeded = false;
  const persist = (key: string, body: ProgressBody) => {
    void outbox
      ?.enqueue(PROGRESS_OUTBOX_KIND, key, body, { merge: mergeProgress })
      .catch(() => {
        /* Storage failure: the in-memory buffer still delivers. */
      });
  };
  const flush = (): Promise<void> => {
    if (running) return running;
    const generation = epoch;
    running = (async () => {
      for (const [key, body] of pending) {
        if (generation !== epoch) break;
        let result: ProgressSendResult = false;
        try {
          result = await send(body);
        } catch {
          /* Retain until reconnect. */
        }
        if (generation !== epoch) break;
        const ok = result !== false;
        if (typeof result === "object" && typeof result.run === "number") {
          run = result.run;
        }
        if (ok && body.restart) restartPending = false;
        // A removed or forbidden issue must not block progress on another.
        if (!ok) continue;
        void outbox
          ?.acknowledge(PROGRESS_OUTBOX_KIND, key, body)
          .catch(() => false);
        if (pending.get(key) === body) pending.delete(key);
        else {
          // Put a newer value at the end so the iterator visits it.
          const newer = pending.get(key)!;
          pending.delete(key);
          pending.set(key, newer);
        }
      }
    })().finally(() => {
      running = undefined;
    });
    return running;
  };
  return {
    set(body: ProgressBody) {
      const tagged: ProgressBody = !seeded
        ? body
        : restartPending
          ? { ...body, run, restart: true }
          : { ...body, run };
      const key = progressOutboxKey(tagged);
      const prev = pending.get(key);
      pending.set(key, prev ? mergeProgress(prev, tagged) : tagged);
      persist(key, tagged);
    },
    flush,
    clear() {
      epoch++;
      pending.clear();
    },
    /** Seed the reading run for the issue now being read. `restart`
     *  makes the next delivered write open a new run. */
    seedRun(nextRun: number, restart: boolean) {
      seeded = true;
      run = nextRun;
      restartPending = restart;
    },
  };
}
