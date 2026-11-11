/**
 * Waiting out a 429 from the server's token buckets
 * (`docs/architecture/rate-limits.md`). Every 429 carries `Retry-After`
 * in whole seconds (at least 1).
 */

/** Shortest wait honoured, whatever the header says. */
export const MIN_RETRY_AFTER_MS = 1_000;
/** Longest wait honoured: a client never parks longer than this per try. */
export const MAX_RETRY_AFTER_MS = 30_000;

/**
 * Milliseconds to wait before retrying a 429, from its `Retry-After`
 * (delta-seconds or an HTTP date), clamped to
 * [`MIN_RETRY_AFTER_MS`, `MAX_RETRY_AFTER_MS`]. A missing or unreadable
 * header waits the minimum.
 */
export function retryAfterMs(
  value: string | null | undefined,
  now: number = Date.now(),
): number {
  let ms = MIN_RETRY_AFTER_MS;
  const v = value?.trim();
  if (v) {
    if (/^\d+$/.test(v)) ms = Number(v) * 1000;
    else {
      const at = Date.parse(v);
      if (Number.isFinite(at)) ms = at - now;
    }
  }
  return Math.min(MAX_RETRY_AFTER_MS, Math.max(MIN_RETRY_AFTER_MS, ms));
}
