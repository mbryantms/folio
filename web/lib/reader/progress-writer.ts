export type ProgressBody = {
  issue_id: string;
  page: number;
  finished?: boolean;
};
/** An in-memory latest-write buffer with one pending value per issue. Serial delivery prevents an
 * older page acknowledgement from clearing or overwriting a newer page. */
export function createProgressWriter(
  send: (body: ProgressBody) => Promise<boolean>,
) {
  const pending = new Map<string, ProgressBody>();
  let running: Promise<void> | undefined;
  let epoch = 0;
  const flush = (): Promise<void> => {
    if (running) return running;
    const generation = epoch;
    running = (async () => {
      for (const [id, body] of pending) {
        if (generation !== epoch) break;
        let ok = false;
        try {
          ok = await send(body);
        } catch {
          /* Retain until reconnect. */
        }
        if (generation !== epoch) break;
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
  };
}
