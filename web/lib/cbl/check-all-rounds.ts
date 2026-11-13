/**
 * "Check for updates" across every reading list, carried on until every
 * list was checked.
 *
 * One `POST /me/cbl-lists/check-all` works under a fixed time budget; once
 * it runs out, the lists it did not start come back with
 * `attempted: false`. This asks again with exactly those ids (`list_ids`)
 * until none are left, and merges the rounds into one result in the
 * lists' original order, as if a single response had covered them all.
 *
 * Stops early when:
 *   - a round checked no list (no progress): the lists still not reached
 *     stay as they are, rows that couldn't be checked, with their Retry;
 *   - a later round fails outright (network, 429 from the import quota,
 *     408): the lists already checked are kept, the rest show that
 *     round's error message.
 * A failure of the first round is the mutation's own error.
 */
import type { CblBulkItemView, CblBulkResultView } from "@/lib/api/types";

/** One `check-all` request. `listIds` undefined ⇒ no body (every list). */
export type CheckAllRequest = (
  listIds?: string[],
) => Promise<CblBulkResultView | null>;

/** Lists checked so far out of all lists in the run. */
export type CheckAllProgress = { checked: number; total: number };

function notAttempted(item: CblBulkItemView): boolean {
  return item.attempted === false;
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
): Promise<CblBulkResultView> {
  const first = await request();
  const order = (first?.items ?? []).map((i) => i.id);
  const byId = new Map<string, CblBulkItemView>(
    (first?.items ?? []).map((i) => [i.id, i]),
  );
  const current = () => order.map((id) => byId.get(id)!);
  let lastRound = first?.items ?? [];

  for (;;) {
    const left = current().filter(notAttempted);
    if (left.length === 0) break;
    // A round that checked nothing would only repeat itself.
    if (!lastRound.some((i) => !notAttempted(i))) break;
    onProgress?.({ checked: order.length - left.length, total: order.length });
    const ids = left.map((i) => i.id);
    let next: CblBulkResultView | null;
    try {
      next = await request(ids);
    } catch (e) {
      const message = e instanceof Error ? e.message : "check failed";
      for (const item of left) {
        byId.set(item.id, { ...item, ok: false, error: message });
      }
      break;
    }
    lastRound = (next?.items ?? []).filter((i) => byId.has(i.id));
    for (const item of lastRound) byId.set(item.id, item);
  }
  return mergedResult(current());
}
