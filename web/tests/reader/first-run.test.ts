/**
 * C5 — the one-time reader first-run overlay flag. Pure storage helper;
 * the node-env harness has no `window`, so we stand up a minimal mock to
 * exercise both the happy path and the private-mode/SSR fail-safe (where
 * we must report "seen" so a broken localStorage never traps the user
 * behind an overlay we can't persist a dismissal for).
 */
import { afterEach, describe, expect, it, vi } from "vitest";

import {
  READER_FIRST_RUN_KEY,
  hasSeenReaderFirstRun,
  markReaderFirstRunSeen,
} from "@/lib/reader/first-run";

function mockWindow(localStorage: Partial<Storage>) {
  vi.stubGlobal("window", { localStorage });
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("reader first-run flag", () => {
  it("reports unseen, then seen after marking", () => {
    const store = new Map<string, string>();
    mockWindow({
      getItem: (k: string) => store.get(k) ?? null,
      setItem: (k: string, v: string) => void store.set(k, v),
    } as Storage);

    expect(hasSeenReaderFirstRun()).toBe(false);
    markReaderFirstRunSeen();
    expect(store.get(READER_FIRST_RUN_KEY)).toBe("1");
    expect(hasSeenReaderFirstRun()).toBe(true);
  });

  it("treats missing window (SSR) as already seen and marking as a no-op", () => {
    // No window stub → typeof window === "undefined".
    expect(hasSeenReaderFirstRun()).toBe(true);
    expect(() => markReaderFirstRunSeen()).not.toThrow();
  });

  it("fails toward 'seen' when storage throws (private mode)", () => {
    mockWindow({
      getItem: () => {
        throw new Error("SecurityError");
      },
      setItem: () => {
        throw new Error("SecurityError");
      },
    } as unknown as Storage);

    expect(hasSeenReaderFirstRun()).toBe(true);
    expect(() => markReaderFirstRunSeen()).not.toThrow();
  });
});
