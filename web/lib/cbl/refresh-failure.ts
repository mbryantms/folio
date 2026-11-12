/**
 * The one place a reading list's refresh failure becomes words: the
 * `/lists` row marker, the In library tooltip and the list page's notice
 * all read from here. Free of React so
 * `tests/library/refresh-failure.test.ts` can pin every kind.
 */
import type {
  CblListView,
  CblRefreshErrorKind,
  CblRefreshFailureView,
} from "@/lib/api/types";

/** What each failure kind means, as the end of a sentence. A `Record`
 *  over the server's enum, so a kind added there fails the type check
 *  until it has words here. */
const REASONS: Record<CblRefreshErrorKind, string> = {
  unreachable: "the source could not be reached",
  http_status: "the source answered with an error",
  missing: "the list is no longer at its source",
  address_refused: "the source address is not allowed",
  too_large: "the file at the source is too large",
  too_many_entries: "the list at the source has grown past the entry limit",
  invalid_list: "the source did not return a valid reading list",
  other: "something went wrong while refreshing it",
};

/** "the source could not be reached", with the server's short detail
 *  in brackets when there is one: "… answered with an error (HTTP 503)". */
export function refreshFailureReason(
  failure: Pick<CblRefreshFailureView, "kind" | "detail">,
): string {
  // `??`: a kind a newer server sends to an older bundle reads as "other".
  const reason = REASONS[failure.kind] ?? REASONS.other;
  return failure.detail ? `${reason} (${failure.detail})` : reason;
}

/** "3 Nov" (the year added when it is not this year). */
export function failureDate(iso: string, now: Date = new Date()): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleDateString(undefined, {
    day: "numeric",
    month: "short",
    ...(d.getFullYear() === now.getFullYear() ? {} : { year: "numeric" }),
  });
}

/** The row marker's name and tooltip: "Refresh failing since 3 Nov: the
 *  source could not be reached". */
export function refreshFailureLabel(
  failure: CblRefreshFailureView,
  now: Date = new Date(),
): string {
  return `Refresh failing since ${failureDate(failure.failing_since, now)}: ${refreshFailureReason(failure)}`;
}

/** What the reader can do about it. `canRefresh`: they may refresh this
 *  list (its owner, or an admin for a shared list). */
export function refreshFailureAdvice(
  kind: CblRefreshErrorKind,
  canRefresh: boolean,
): string {
  if (!canRefresh) {
    return "The list keeps its last good copy. Whoever manages it can check its source.";
  }
  switch (kind) {
    case "unreachable":
    case "http_status":
      return "This is often temporary: try Refresh now. If it keeps failing, check that the source address still works.";
    case "missing":
      return "Check where the list lives now; to follow a new address, import the list again from there.";
    case "address_refused":
      return "Folio does not fetch from private or local addresses. Import the list again from a public https address.";
    case "too_large":
    case "too_many_entries":
      return "The list keeps its last good copy. Split the list at the source, or keep this copy and set the schedule to Manual only.";
    case "invalid_list":
      return "Check that the source address still points at a .cbl file, then try Refresh now.";
    case "other":
    default:
      // `default`: a kind a newer server sends to an older bundle.
      return "Try Refresh now. If it keeps failing, check the source address.";
  }
}

/** "3 failed attempts in a row" / "1 failed attempt". */
export function failureCountLabel(count: number): string {
  return count === 1 ? "1 failed attempt" : `${count} failed attempts in a row`;
}

/** The list's date for the In library tooltip, in the words the row
 *  uses: "Updated …" for a refresh, "Imported …" otherwise. A failing
 *  list says when it last updated *successfully*, so the tooltip never
 *  claims a fresh update. */
export function listDateVerb(
  list: Pick<CblListView, "last_refreshed_at" | "refresh_failure">,
): string {
  if (!list.last_refreshed_at) return "Imported";
  return list.refresh_failure ? "Last updated" : "Updated";
}
