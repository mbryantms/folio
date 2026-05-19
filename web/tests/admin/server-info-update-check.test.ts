/**
 * Unit tests for the version-comparison helpers in `<ServerInfoClient>`
 * (server-info-github-link 1.0 M4).
 *
 * The contract this pins:
 *   - `parseSemverPrefix` extracts the `vX.Y.Z` integer tuple from
 *     clean tags AND from `git describe` extensions (the extensions
 *     are ignored at parse time; the comparison helper handles them).
 *   - `isNewerRelease` returns `true` only for a strictly newer
 *     upstream tag. Equal-or-older tags, malformed inputs, and
 *     `null` upstream all return `false`. Special case: when the
 *     current side carries `-N-gSHA` extensions, the user is past
 *     the matching upstream tag — so an upstream === current tag is
 *     NOT newer.
 */
import { describe, expect, it } from "vitest";

import {
  isNewerRelease,
  parseSemverPrefix,
} from "@/components/admin/observability/ServerInfoClient";
import type { LatestReleaseView } from "@/lib/api/types";

function release(tag: string): LatestReleaseView {
  return {
    tag,
    html_url: `https://github.com/x/y/releases/tag/${tag}`,
    published_at: "2026-05-17T00:00:00Z",
  };
}

describe("parseSemverPrefix", () => {
  it("parses clean tags", () => {
    expect(parseSemverPrefix("v0.1.8")).toEqual([0, 1, 8]);
    expect(parseSemverPrefix("v1.0.0")).toEqual([1, 0, 0]);
    expect(parseSemverPrefix("v2")).toEqual([2]);
    expect(parseSemverPrefix("v1.0.0.1")).toEqual([1, 0, 0, 1]);
  });

  it("strips git-describe extensions", () => {
    expect(parseSemverPrefix("v0.1.8-3-gabcd1234")).toEqual([0, 1, 8]);
    expect(parseSemverPrefix("v0.1.8-dirty")).toEqual([0, 1, 8]);
  });

  it("returns null for shapes we can't compare", () => {
    expect(parseSemverPrefix("dev")).toBeNull();
    expect(parseSemverPrefix("abcd1234")).toBeNull();
    expect(parseSemverPrefix("0.1.8")).toBeNull();
    expect(parseSemverPrefix("")).toBeNull();
  });
});

describe("isNewerRelease", () => {
  it("returns false when latest is null", () => {
    expect(isNewerRelease("v0.1.8", null)).toBe(false);
  });

  it("returns false when current can't be parsed", () => {
    expect(isNewerRelease("dev", release("v0.1.9"))).toBe(false);
    expect(isNewerRelease("abcd1234", release("v0.1.9"))).toBe(false);
  });

  it("returns false when latest tag can't be parsed", () => {
    expect(isNewerRelease("v0.1.8", release("not-a-tag"))).toBe(false);
  });

  it("returns true for strictly newer patch / minor / major", () => {
    expect(isNewerRelease("v0.1.8", release("v0.1.9"))).toBe(true);
    expect(isNewerRelease("v0.1.8", release("v0.2.0"))).toBe(true);
    expect(isNewerRelease("v0.1.8", release("v1.0.0"))).toBe(true);
  });

  it("returns false for equal or older tags", () => {
    expect(isNewerRelease("v0.1.8", release("v0.1.8"))).toBe(false);
    expect(isNewerRelease("v0.1.8", release("v0.1.7"))).toBe(false);
    expect(isNewerRelease("v0.1.8", release("v0.0.99"))).toBe(false);
  });

  it("treats a user past a tag (with git-describe extensions) as already up-to-date for that tag", () => {
    // The user is on v0.1.8 plus 3 extra commits — upstream v0.1.8
    // is NOT newer.
    expect(isNewerRelease("v0.1.8-3-gabcd1234", release("v0.1.8"))).toBe(false);
  });

  it("still flags a strictly newer release even when current has extensions", () => {
    expect(isNewerRelease("v0.1.8-3-gabcd1234", release("v0.1.9"))).toBe(true);
    expect(isNewerRelease("v0.1.8-dirty", release("v0.2.0"))).toBe(true);
  });

  it("handles different tuple lengths gracefully", () => {
    // v0.1 vs v0.1.1 — missing component reads as 0.
    expect(isNewerRelease("v0.1", release("v0.1.1"))).toBe(true);
    expect(isNewerRelease("v0.1.1", release("v0.1"))).toBe(false);
  });
});
