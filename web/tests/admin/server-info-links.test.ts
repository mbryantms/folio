/**
 * Unit tests for the link helpers in `<ServerInfoClient>` (M1+M2 of
 * server-info-github-link 1.0).
 *
 * The contract this pins:
 *   - `releaseUrl` ONLY produces a link for clean `vX.Y.Z` tags. The
 *     `git describe` extensions (`v0.1.8-3-gabcd1234`, `v0.1.8-dirty`,
 *     bare SHA) don't have release pages, so they stay plain text.
 *   - `commitUrl` works for any non-"unknown" SHA when a repo URL is
 *     present.
 *   - `repoDisplay` strips the protocol for a compact label.
 *   - `formatRelativeFromEpoch` produces stable human-readable strings
 *     across the relevant buckets (minutes → years).
 */
import { describe, expect, it } from "vitest";

import {
  commitUrl,
  formatRelativeFromEpoch,
  releaseUrl,
  repoDisplay,
} from "@/components/admin/observability/ServerInfoClient";

const REPO = "https://github.com/mbryantms/folio";

describe("releaseUrl", () => {
  it("returns undefined when repo_url is null", () => {
    expect(releaseUrl(null, "v0.1.8")).toBeUndefined();
  });

  it("links clean tagged versions", () => {
    expect(releaseUrl(REPO, "v0.1.8")).toBe(
      "https://github.com/mbryantms/folio/releases/tag/v0.1.8",
    );
    expect(releaseUrl(REPO, "v1.0.0")).toBe(
      "https://github.com/mbryantms/folio/releases/tag/v1.0.0",
    );
  });

  it("rejects git-describe extensions and bare SHAs", () => {
    // 3 commits past a tag — would 404 on GitHub.
    expect(releaseUrl(REPO, "v0.1.8-3-gabcd1234")).toBeUndefined();
    // Dirty release — also no matching page.
    expect(releaseUrl(REPO, "v0.1.8-dirty")).toBeUndefined();
    // Bare SHA (no tags exist).
    expect(releaseUrl(REPO, "abcd1234")).toBeUndefined();
    // Build script fallback.
    expect(releaseUrl(REPO, "dev")).toBeUndefined();
    // Missing leading `v`.
    expect(releaseUrl(REPO, "0.1.8")).toBeUndefined();
  });

  it("accepts patch and pre-release-free shapes only", () => {
    expect(releaseUrl(REPO, "v1")).toBe(
      "https://github.com/mbryantms/folio/releases/tag/v1",
    );
    expect(releaseUrl(REPO, "v1.0")).toBe(
      "https://github.com/mbryantms/folio/releases/tag/v1.0",
    );
    expect(releaseUrl(REPO, "v1.0.0.1")).toBe(
      "https://github.com/mbryantms/folio/releases/tag/v1.0.0.1",
    );
  });
});

describe("commitUrl", () => {
  it("returns undefined when repo_url is null", () => {
    expect(commitUrl(null, "abcd1234")).toBeUndefined();
  });

  it("returns undefined for the 'unknown' fallback", () => {
    expect(commitUrl(REPO, "unknown")).toBeUndefined();
    expect(commitUrl(REPO, "")).toBeUndefined();
  });

  it("links the full SHA when both are present", () => {
    expect(
      commitUrl(REPO, "abcd1234ef567890abcd1234ef567890abcd1234"),
    ).toBe(
      "https://github.com/mbryantms/folio/commit/abcd1234ef567890abcd1234ef567890abcd1234",
    );
  });
});

describe("repoDisplay", () => {
  it("strips https:// for the label", () => {
    expect(repoDisplay(REPO)).toBe("github.com/mbryantms/folio");
  });

  it("strips http:// for the label", () => {
    expect(repoDisplay("http://gitlab.local/foo/bar")).toBe(
      "gitlab.local/foo/bar",
    );
  });

  it("returns null when repo_url is null", () => {
    expect(repoDisplay(null)).toBeNull();
  });
});

describe("formatRelativeFromEpoch", () => {
  // Fixed clock for deterministic assertions.
  const NOW = new Date("2026-05-17T12:00:00Z").getTime();

  it("'just now' for the current second + the future", () => {
    expect(formatRelativeFromEpoch(NOW / 1000, NOW)).toBe("just now");
    expect(formatRelativeFromEpoch((NOW + 5_000) / 1000, NOW)).toBe(
      "just now",
    );
  });

  it("minutes bucket", () => {
    expect(formatRelativeFromEpoch((NOW - 5 * 60_000) / 1000, NOW)).toBe(
      "5m ago",
    );
    expect(formatRelativeFromEpoch((NOW - 59 * 60_000) / 1000, NOW)).toBe(
      "59m ago",
    );
  });

  it("hours bucket", () => {
    expect(formatRelativeFromEpoch((NOW - 3 * 3600_000) / 1000, NOW)).toBe(
      "3h ago",
    );
    expect(formatRelativeFromEpoch((NOW - 23 * 3600_000) / 1000, NOW)).toBe(
      "23h ago",
    );
  });

  it("days bucket", () => {
    expect(
      formatRelativeFromEpoch((NOW - 2 * 24 * 3600_000) / 1000, NOW),
    ).toBe("2d ago");
    expect(
      formatRelativeFromEpoch((NOW - 29 * 24 * 3600_000) / 1000, NOW),
    ).toBe("29d ago");
  });

  it("months bucket", () => {
    expect(
      formatRelativeFromEpoch((NOW - 60 * 24 * 3600_000) / 1000, NOW),
    ).toBe("2mo ago");
  });

  it("years bucket", () => {
    expect(
      formatRelativeFromEpoch((NOW - 400 * 24 * 3600_000) / 1000, NOW),
    ).toBe("1y ago");
  });
});
