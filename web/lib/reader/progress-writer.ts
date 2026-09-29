export type ProgressBody = {
  issue_id: string;
  page: number;
  finished?: boolean;
  /** Reading run this write belongs to. Omitted on a `restart` write. */
  run?: number;
  /** Open a new reading run at `page` ("Read from beginning", or a
   *  finished issue reopened from the cover). */
  restart?: boolean;
};
/** What `send` resolves to: `false` when the server rejected the write,
 * `true` or `{ run }` when it accepted it (the reply's reading run, when
 * the caller could read it). */
export type ProgressSendResult = boolean | { run?: number };

/** An in-memory latest-write buffer with one pending value per issue. Serial delivery prevents an
 * older page acknowledgement from clearing or overwriting a newer page.
 *
 * Reading runs (WP-1.3): the writer owns the run it is writing into and
 * whether the next delivered write must open a new one (`seedRun`). Bodies
 * are tagged at flush time — `restart: true` on the first delivery after a
 * restart seed, `run` otherwise — and the run is adopted from each reply. */
export function createProgressWriter(
  send: (body: ProgressBody) => Promise<ProgressSendResult>,
) {
  const pending = new Map<string, ProgressBody>();
  let running: Promise<void> | undefined;
  let epoch = 0;
  let run = 0;
  let restartPending = false;
  let seeded = false;
  const flush = (): Promise<void> => {
    if (running) return running;
    const generation = epoch;
    running = (async () => {
      for (const [id, body] of pending) {
        if (generation !== epoch) break;
        const tagged: ProgressBody = !seeded
          ? body
          : restartPending
            ? { ...body, restart: true }
            : { ...body, run };
        let result: ProgressSendResult = false;
        try {
          result = await send(tagged);
        } catch {
          /* Retain until reconnect. */
        }
        if (generation !== epoch) break;
        const ok = result !== false;
        if (typeof result === "object" && typeof result.run === "number") {
          run = result.run;
        }
        if (ok && tagged.restart) restartPending = false;
        // A removed or forbidden issue must not block progress on another.
        if (!ok) continue;
        if (pending.get(id) === body) pending.delete(id);
        else {
          // Put a newer value at the end so the iterator visits it.
          const newer = pending.get(id)!;
          pending.delete(id);
          pending.set(id, newer);
        }
      }
    })().finally(() => {
      running = undefined;
    });
    return running;
  };
  return {
    set(body: ProgressBody) {
      pending.set(body.issue_id, body);
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
