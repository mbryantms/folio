/**
 * "Check for updates" across every reading list, carried on until every
 * list was checked.
 *
 * One `POST /me/cbl-lists/check-all` works under a fixed time budget; once
 * it runs out, the lists it did not start come back with
 * `attempted: false`. This asks again with those ids (`list_ids`, at most
 * `MAX_IDS_PER_ROUND` per round) until none are left, and merges the
 * rounds into one result in the lists' original order, as if a single
 * response had covered them all. The server charges the whole run one
 * import token (`cbl::quota` continuation allowance).
 *
 * Stops early when:
 *   - a round leaves as many lists unchecked as before it (no progress):
 *     those stay as they are, rows that couldn't be checked, with their
 *     Retry;
 *   - a later round fails outright (network, 429 from the import quota,
 *     408): the lists already checked are kept, the rest show that
 *     round's error message;
 *   - as a last guard, after as many rounds as there are lists;
 *   - `signal` was aborted (the page went away): no further round is
 *     sent and the run rejects with an `AbortError`.
 * A failure of the first round is the mutation's own error.
 */
import type { CblBulkItemView, CblBulkResultView } from "@/lib/api/types";

/** Most ids one round sends. Mirrors the server's
 *  `api::cbl_lists::MAX_BULK_LIST_IDS`, which refuses more with a 422. */
export const MAX_IDS_PER_ROUND = 1000;

/** One `check-all` request. `listIds` undefined ⇒ no body (every list). */
export type CheckAllRequest = (
  listIds?: string[],
) => Promise<CblBulkResultView | null>;

/** Lists checked so far out of all lists in the run. */
export type CheckAllProgress = { checked: number; total: number };

function notAttempted(item: CblBulkItemView): boolean {
  return item.attempted === false;
}

function abandoned(): DOMException {
  return new DOMException("the check was abandoned", "AbortError");
}

export function mergedResult(items: CblBulkItemView[]): CblBulkResultView {
  return {
    items,
    changed_count: items.filter((i) => i.changed).length,
    failed_count: items.filter((i) => !i.ok).length,
  };
}

export async function checkAllUntilDone(
  request: CheckAllRequest,
  onProgress?: (p: CheckAllProgress) => void,
  signal?: AbortSignal,
): Promise<CblBulkResultView> {
  const first = await request();
  if (signal?.aborted) throw abandoned();
  const firstItems = first?.items ?? [];
  const order = firstItems.map((i) => i.id);
  const byId = new Map<string, CblBulkItemView>(
    firstItems.map((i) => [i.id, i]),
  );
  const current = () => order.map((id) => byId.get(id)!);
  const unchecked = () => current().filter(notAttempted);
  const total = order.length;

  // Lists still unchecked before the last round; the first round started
  // from all of them.
  let before = total;
  let left = unchecked();
  for (let round = 0; round < total; round++) {
    if (left.length === 0 || left.length >= before) break;
    onProgress?.({ checked: total - left.length, total });
    const asked = left.slice(0, MAX_IDS_PER_ROUND).map((i) => i.id);
    let next: CblBulkResultView | null;
    try {
      next = await request(asked);
    } catch (e) {
      if (signal?.aborted) throw abandoned();
      const message = e instanceof Error ? e.message : "check failed";
      for (const item of left) {
        byId.set(item.id, { ...item, ok: false, error: message });
      }
      break;
    }
    if (signal?.aborted) throw abandoned();
    // Only answers to this round's question count: an id the round did not
    // ask about (e.g. one already resolved) never overwrites its row.
    const askedSet = new Set(asked);
    for (const item of next?.items ?? []) {
      if (askedSet.has(item.id)) byId.set(item.id, item);
    }
    before = left.length;
    left = unchecked();
  }
  return mergedResult(current());
}
