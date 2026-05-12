import { describe, expect, it } from "vitest";

import { prettyUserAgent, timeAgo } from "@/lib/sessions";

describe("prettyUserAgent", () => {
  it("returns 'Unknown device' for null / empty input", () => {
    expect(prettyUserAgent(null).device).toBe("Unknown device");
    expect(prettyUserAgent("").device).toBe("Unknown device");
    expect(prettyUserAgent(undefined).device).toBe("Unknown device");
  });

  it("classifies Chrome on macOS", () => {
    const ua =
      "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";
    expect(prettyUserAgent(ua).device).toBe("Chrome on macOS");
  });

  it("classifies Firefox on Linux", () => {
    const ua =
      "Mozilla/5.0 (X11; Linux x86_64; rv:120.0) Gecko/20100101 Firefox/120.0";
    expect(prettyUserAgent(ua).device).toBe("Firefox on Linux");
  });

  it("classifies Safari on iOS (and not Chrome)", () => {
    const ua =
      "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1";
    expect(prettyUserAgent(ua).device).toBe("Safari on iOS");
  });

  it("prefers Edge over the embedded Chrome/ token", () => {
    const ua =
      "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36 Edg/120.0.0.0";
    expect(prettyUserAgent(ua).device).toBe("Edge on Windows");
  });

  it("classifies curl/wget by token", () => {
    expect(prettyUserAgent("curl/8.4.0").device).toBe("curl on Unknown OS");
    expect(prettyUserAgent("Wget/1.21.4").device).toBe("wget on Unknown OS");
  });

  it("preserves the raw UA so the row can render a tooltip", () => {
    const ua = "Mozilla/5.0 (Macintosh) Chrome/120.0";
    expect(prettyUserAgent(ua).raw).toBe(ua);
  });
});

describe("timeAgo", () => {
  const NOW = new Date("2026-05-12T12:00:00Z").getTime();

  it("returns 'just now' under 60s", () => {
    expect(timeAgo("2026-05-12T11:59:30Z", NOW)).toBe("just now");
  });

  it("formats minutes", () => {
    expect(timeAgo("2026-05-12T11:55:00Z", NOW)).toBe("5 mins ago");
    expect(timeAgo("2026-05-12T11:59:00Z", NOW)).toBe("1 min ago");
  });

  it("formats hours", () => {
    expect(timeAgo("2026-05-12T09:00:00Z", NOW)).toBe("3 hours ago");
    expect(timeAgo("2026-05-12T11:00:00Z", NOW)).toBe("1 hour ago");
  });

  it("formats days", () => {
    expect(timeAgo("2026-05-10T12:00:00Z", NOW)).toBe("2 days ago");
    expect(timeAgo("2026-05-11T12:00:00Z", NOW)).toBe("1 day ago");
  });

  it("falls back to a locale date past 30 days", () => {
    const result = timeAgo("2026-04-01T12:00:00Z", NOW);
    expect(result).not.toMatch(/(ago|just now)/);
  });

  it("returns the input verbatim for invalid dates", () => {
    expect(timeAgo("not-a-date", NOW)).toBe("not-a-date");
  });

  it("clamps negative diffs (clock skew) to 'just now'", () => {
    expect(timeAgo("2026-05-12T12:00:30Z", NOW)).toBe("just now");
  });
});
