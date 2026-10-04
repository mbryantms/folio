import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { buildId, stampEntries } from "../../serwist.config.js";

describe("serwist precache manifest", () => {
  const entries = [{ url: "offline.html", revision: "abc123", size: 900 }];

  it("folds the build id into each revision so every release changes sw.js", () => {
    const a = stampEntries(entries, "build-a");
    const b = stampEntries(entries, "build-b");
    expect(a).toEqual([
      { url: "/offline.html", revision: "abc123-build-a", size: 900 },
    ]);
    expect(a[0]?.revision).not.toBe(b[0]?.revision);
  });

  it("keeps the content revision when no build id is available", () => {
    expect(stampEntries(entries, null)).toEqual([
      { url: "/offline.html", revision: "abc123", size: 900 },
    ]);
  });

  it("reads Next's BUILD_ID and tolerates its absence", () => {
    const dir = mkdtempSync(join(tmpdir(), "folio-sw-"));
    const file = join(dir, "BUILD_ID");
    writeFileSync(file, "xYz-123\n");
    expect(buildId(file)).toBe("xYz-123");
    expect(buildId(join(dir, "missing"))).toBeNull();
  });
});
