/**
 * A reading list's refresh failure in words: every kind the server can
 * send has a reason and advice, the row marker's label, and the In
 * library tooltip that must not claim a fresh update for a failing list.
 */
import { describe, expect, it } from "vitest";

import type {
  CblRefreshErrorKind,
  CblRefreshFailureView,
} from "@/lib/api/types";
import { listDateTitle } from "@/lib/cbl/list-rows";
import {
  failureCountLabel,
  failureDate,
  listDateVerb,
  refreshFailureAdvice,
  refreshFailureLabel,
  refreshFailureReason,
} from "@/lib/cbl/refresh-failure";

/** Every kind, as a `Record` so a kind the server adds fails the type
 *  check here too until it has a row. */
const EXPECTED: Record<CblRefreshErrorKind, string> = {
  unreachable: "the source could not be reached",
  http_status: "the source answered with an error",
  missing: "the list is no longer at its source",
  address_refused: "the source address is not allowed",
  too_large: "the file at the source is too large",
  too_many_entries: "the list at the source has grown past the entry limit",
  invalid_list: "the source did not return a valid reading list",
  other: "something went wrong while refreshing it",
};
const KINDS = Object.keys(EXPECTED) as CblRefreshErrorKind[];

const NOW = new Date("2026-11-12T12:00:00Z");

function failure(
  over: Partial<CblRefreshFailureView> = {},
): CblRefreshFailureView {
  return {
    kind: "unreachable",
    detail: null,
    failing_since: "2026-11-03T09:00:00Z",
    consecutive_failures: 3,
    ...over,
  };
}

describe("refresh failure words", () => {
  it.each(KINDS)("%s has a reason and advice for both audiences", (kind) => {
    expect(refreshFailureReason({ kind, detail: null })).toBe(EXPECTED[kind]);
    const manager = refreshFailureAdvice(kind, true);
    const reader = refreshFailureAdvice(kind, false);
    expect(manager.length).toBeGreaterThan(10);
    expect(reader).toMatch(/last good copy/);
    expect(manager).not.toBe(reader);
  });

  it("adds the server's detail in brackets", () => {
    expect(
      refreshFailureReason({ kind: "http_status", detail: "HTTP 503" }),
    ).toBe("the source answered with an error (HTTP 503)");
  });

  it("labels the row marker with the start of the failing run", () => {
    const day = failureDate("2026-11-03T09:00:00Z", NOW);
    expect(day).not.toMatch(/2026/); // same year: no year
    expect(refreshFailureLabel(failure(), NOW)).toBe(
      `Refresh failing since ${day}: the source could not be reached`,
    );
    expect(failureDate("2025-11-03T09:00:00Z", NOW)).toMatch(/2025/);
  });

  it("counts failed attempts", () => {
    expect(failureCountLabel(1)).toBe("1 failed attempt");
    expect(failureCountLabel(5)).toBe("5 failed attempts in a row");
  });
});

describe("In library tooltip", () => {
  const base = {
    imported_at: "2025-01-01T00:00:00Z",
    last_refreshed_at: "2026-11-09T12:00:00Z",
    refresh_failure: null,
  };

  it("says Updated for a healthy refreshed list, Imported before any refresh", () => {
    expect(listDateVerb(base)).toBe("Updated");
    expect(listDateTitle(base, NOW)).toMatch(/^Updated 3 days ago \(/);
    const imported = { ...base, last_refreshed_at: null };
    expect(listDateVerb(imported)).toBe("Imported");
    expect(listDateTitle(imported, NOW)).toMatch(/^Imported /);
  });

  it("does not claim a fresh update for a failing list", () => {
    const failing = { ...base, refresh_failure: failure() };
    const title = listDateTitle(failing, NOW) ?? "";
    expect(title).toMatch(/^Last updated 3 days ago \(/);
    expect(title).toContain(
      `. Refresh failing since ${failureDate("2026-11-03T09:00:00Z", NOW)}`,
    );
  });
});
