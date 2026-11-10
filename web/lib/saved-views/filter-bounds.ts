/**
 * Size caps on a saved-view filter, mirrored by hand from the server's
 * `MAX_FILTER_CONDITIONS` / `MAX_FILTER_LIST_VALUES` /
 * `MAX_FILTER_VALUE_LEN` in `crates/server/src/views/dsl.rs`. The server
 * enforces them on create, update and both previews (422 with
 * `error.details`); the builder uses them to stop the user before that.
 * Keep the two in step.
 */
export const MAX_FILTER_CONDITIONS = 50;
export const MAX_FILTER_LIST_VALUES = 200;
/** UTF-8 bytes on the server; the builder treats it as characters. */
export const MAX_FILTER_VALUE_LEN = 200;

/** `error.code` of the 422 a filter query answers when it hits the
 *  server's statement timeout (`views::timebox::FILTER_TOO_SLOW_CODE`). */
export const FILTER_TOO_SLOW_CODE = "filter_too_slow";

/** Shown when a filter query timed out; the server sends the same text. */
export const FILTER_TOO_SLOW_MESSAGE =
  "This filter takes too long to run. Remove some conditions and try again.";

/** Inline-notice text for a failed results / preview fetch: the
 *  time-box message when the server says the filter is too slow, the
 *  server's own message for any other 422, else a generic line. */
export function filterQueryErrorMessage(err: unknown): string {
  if (err && typeof err === "object") {
    const e = err as { code?: unknown; status?: unknown; message?: unknown };
    if (e.code === FILTER_TOO_SLOW_CODE) return FILTER_TOO_SLOW_MESSAGE;
    if (e.status === 422 && typeof e.message === "string" && e.message) {
      return e.message;
    }
  }
  return "Couldn't load this view. Try again later.";
}
