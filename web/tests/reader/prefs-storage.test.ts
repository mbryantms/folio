import { beforeEach, describe, expect, it } from "vitest";

import {
  DEFAULT_SCOPE,
  MAX_SERIES_BUCKETS,
  prefGet,
  prefSet,
} from "@/lib/reader/prefs-storage";

/** Minimal in-memory `Storage` so the core logic tests without a DOM. */
class FakeStorage implements Storage {
  private map = new Map<string, string>();
  get length() {
    return this.map.size;
  }
  clear() {
    this.map.clear();
  }
  getItem(k: string) {
    return this.map.has(k) ? this.map.get(k)! : null;
  }
  setItem(k: string, v: string) {
    this.map.set(k, String(v));
  }
  removeItem(k: string) {
    this.map.delete(k);
  }
  key(i: number) {
    return [...this.map.keys()][i] ?? null;
  }
  /** Test helper: every key currently held. */
  keys() {
    return [...this.map.keys()];
  }
}

let store: FakeStorage;
beforeEach(() => {
  store = new FakeStorage();
});

describe("prefSet / prefGet", () => {
  it("round-trips a per-series value under the versioned namespace", () => {
    prefSet(store, "viewMode", "ser-1", "double");
    expect(prefGet(store, "viewMode", "ser-1")).toBe("double");
    expect(store.keys()).toContain("reader.v1:viewMode:ser-1");
    // No bare legacy key is written.
    expect(store.keys()).not.toContain("reader:viewMode:ser-1");
  });

  it("keeps scopes isolated", () => {
    prefSet(store, "fitMode", "ser-1", "width");
    prefSet(store, "fitMode", "ser-2", "height");
    expect(prefGet(store, "fitMode", "ser-1")).toBe("width");
    expect(prefGet(store, "fitMode", "ser-2")).toBe("height");
  });
});

describe("legacy migration", () => {
  it("imports unversioned keys once, preserving values, then drops them", () => {
    store.setItem("reader:viewMode:ser-1", "webtoon");
    store.setItem("reader:brightness:_default", "1.2");
    // First access triggers migration.
    expect(prefGet(store, "viewMode", "ser-1")).toBe("webtoon");
    expect(prefGet(store, "brightness", DEFAULT_SCOPE)).toBe("1.2");
    expect(store.keys()).not.toContain("reader:viewMode:ser-1");
    expect(store.keys()).not.toContain("reader:brightness:_default");
    expect(store.getItem("reader.v1:__migrated")).toBe("1");
  });

  it("does not overwrite an already-migrated value on a later run", () => {
    store.setItem("reader:fitMode:ser-1", "original");
    prefSet(store, "fitMode", "ser-1", "width"); // migrates, then writes
    // Legacy is gone and the new write stands.
    expect(prefGet(store, "fitMode", "ser-1")).toBe("width");
  });
});

describe("per-series LRU eviction", () => {
  it("evicts the least-recently-written series past the cap", () => {
    // Write one bucket per series, oldest first.
    for (let i = 0; i < MAX_SERIES_BUCKETS + 5; i++) {
      prefSet(store, "viewMode", `ser-${i}`, "single");
    }
    // The 5 oldest buckets are gone; the newest cap-worth survive.
    for (let i = 0; i < 5; i++) {
      expect(prefGet(store, "viewMode", `ser-${i}`)).toBeNull();
    }
    for (let i = 5; i < MAX_SERIES_BUCKETS + 5; i++) {
      expect(prefGet(store, "viewMode", `ser-${i}`)).toBe("single");
    }
  });

  it("a re-write refreshes recency so the bucket survives", () => {
    for (let i = 0; i < MAX_SERIES_BUCKETS; i++) {
      prefSet(store, "viewMode", `ser-${i}`, "single");
    }
    // Touch the oldest, then push one more over the cap.
    prefSet(store, "viewMode", "ser-0", "double");
    prefSet(store, "viewMode", "ser-new", "single");
    // ser-0 was refreshed; ser-1 (now oldest) is evicted instead.
    expect(prefGet(store, "viewMode", "ser-0")).toBe("double");
    expect(prefGet(store, "viewMode", "ser-1")).toBeNull();
  });

  it("never evicts the global default scope", () => {
    prefSet(store, "brightness", DEFAULT_SCOPE, "1.3");
    for (let i = 0; i < MAX_SERIES_BUCKETS + 10; i++) {
      prefSet(store, "viewMode", `ser-${i}`, "single");
    }
    expect(prefGet(store, "brightness", DEFAULT_SCOPE)).toBe("1.3");
  });
});
