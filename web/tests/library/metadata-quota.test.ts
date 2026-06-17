/**
 * Pure-helper unit tests for the provider-quota formatters (audit B13).
 */
import { describe, expect, it } from "vitest";

import {
  formatCountdown,
  formatRetryEta,
  isDepleted,
  providerLabel,
  summarizeProviderQuota,
} from "@/lib/metadata/quota";

describe("formatCountdown", () => {
  it("renders the relevant unit", () => {
    expect(formatCountdown(0)).toBe("now");
    expect(formatCountdown(-5)).toBe("now");
    expect(formatCountdown(20)).toBe("<1m");
    expect(formatCountdown(60 * 47)).toBe("47m");
    expect(formatCountdown(60 * 60 * 2 + 60 * 3)).toBe("2h 3m");
    expect(formatCountdown(60 * 60 * 2)).toBe("2h");
  });
});

describe("isDepleted", () => {
  it("is true when either bucket hits zero", () => {
    expect(isDepleted({ provider: "comicvine", remaining_hour: 0 })).toBe(true);
    expect(
      isDepleted({ provider: "metron", remaining_hour: 10, remaining_day: 0 }),
    ).toBe(true);
    expect(
      isDepleted({ provider: "metron", remaining_hour: 10, remaining_day: 4 }),
    ).toBe(false);
  });
});

describe("summarizeProviderQuota", () => {
  it("shows the hour bucket for ComicVine", () => {
    expect(
      summarizeProviderQuota({ provider: "comicvine", remaining_hour: 180 }),
    ).toBe("ComicVine: 180/hr");
  });

  it("shows both buckets for Metron and a reset when depleted", () => {
    expect(
      summarizeProviderQuota({
        provider: "metron",
        remaining_hour: 0,
        remaining_day: 4980,
        seconds_until_reset: 60 * 12,
      }),
    ).toBe("Metron: 0/hr · 4,980/day (resets in 12m)");
  });

  it("omits the reset when budget remains", () => {
    expect(
      summarizeProviderQuota({
        provider: "comicvine",
        remaining_hour: 50,
        seconds_until_reset: 600,
      }),
    ).toBe("ComicVine: 50/hr");
  });

  it("falls back to a dash with no numbers", () => {
    expect(summarizeProviderQuota({ provider: "comicvine" })).toBe(
      "ComicVine: —",
    );
  });
});

describe("formatRetryEta", () => {
  it("returns null without a retry interval", () => {
    expect(formatRetryEta(null)).toBeNull();
    expect(formatRetryEta(undefined)).toBeNull();
  });

  it("formats the relative interval", () => {
    expect(formatRetryEta(60 * 47)).toBe("47m");
    expect(formatRetryEta(0)).toBe("now");
  });
});

describe("providerLabel", () => {
  it("maps known ids and falls back to the raw id", () => {
    expect(providerLabel("comicvine")).toBe("ComicVine");
    expect(providerLabel("metron")).toBe("Metron");
    expect(providerLabel("gcd")).toBe("gcd");
  });
});
