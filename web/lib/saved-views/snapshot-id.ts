/**
 * The id a server render gives its prefetched snapshot
 * (`prefetchPageRails`), read back by `PrefetchBoundary` in the browser.
 *
 * `<server render time, base 36>.<random>`: the random part makes it
 * unique; the time part lets the browser order snapshots by *server*
 * time, so it can tell a replayed old page from a new render even after
 * it has forgotten the id (see `snapshotFirstSeen`). It is never compared
 * with the browser's clock.
 */
export function newSnapshotId(now: number = Date.now()): string {
  return `${Math.floor(now).toString(36)}.${crypto.randomUUID()}`;
}

/** The server render time in `id`, or `null` for an id of another shape. */
export function snapshotRenderedAt(id: string): number | null {
  const dot = id.indexOf(".");
  if (dot <= 0) return null;
  const head = id.slice(0, dot);
  if (!/^[0-9a-z]+$/.test(head)) return null;
  const ms = Number.parseInt(head, 36);
  return Number.isSafeInteger(ms) ? ms : null;
}
