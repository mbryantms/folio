import { describe, expect, it } from "vitest";

import {
  applyScanEvent,
  emptyScan,
  formatEta,
  humanizeKind,
  seedFromActive,
  sparkPath,
} from "@/lib/admin/live-scan";
import type { ScanEvent } from "@/lib/api/types";

const LIB = "lib-1";
const progress = (
  over: Partial<Extract<ScanEvent, { type: "scan.progress" }>>,
): ScanEvent =>
  ({
    type: "scan.progress",
    library_id: LIB,
    scan_id: "s1",
    kind: "library",
    phase: "scanning",
    unit: "files",
    completed: 0,
    total: 100,
    current_label: null,
    files_seen: 0,
    files_added: 0,
    files_updated: 0,
    files_unchanged: 0,
    files_skipped: 0,
    files_duplicate: 0,
    issues_removed: 0,
    health_issues: 0,
    series_scanned: 0,
    series_total: 0,
    series_skipped_unchanged: 0,
    files_total: 0,
    root_files: 0,
    empty_folders: 0,
    ...over,
  }) as ScanEvent;

describe("live scan reducer", () => {
  it("ignores other libraries", () => {
    const s = emptyScan(LIB);
    expect(
      applyScanEvent(s, progress({ library_id: "other", completed: 5 }), 0),
    ).toBe(s);
  });

  it("tracks counters, a client rate and an ETA from progress", () => {
    let s = applyScanEvent(
      emptyScan(LIB),
      {
        type: "scan.started",
        library_id: LIB,
        scan_id: "s1",
        at: "x",
      } as ScanEvent,
      0,
    );
    // 10 files/s, 10 units/s.
    for (let i = 1; i <= 5; i++) {
      s = applyScanEvent(
        s,
        progress({ completed: i * 10, files_seen: i * 10, files_added: i }),
        i * 1000,
      );
    }
    expect(s.status).toBe("running");
    expect(s.counters).toMatchObject({ files: 50, newIssues: 5 });
    expect(s.rate).toBeCloseTo(10, 0);
    // 50 units left at 10/s.
    expect(s.etaMs).toBeCloseTo(5000, -2);
    expect(s.samples.length).toBe(5);
  });

  it("prefers the server's ETA when it sends one", () => {
    const s = applyScanEvent(
      emptyScan(LIB),
      progress({ completed: 10, eta_ms: 1234 } as never),
      1000,
    );
    expect(s.etaMs).toBe(1234);
  });

  it("collects discoveries once, counting only new series", () => {
    let s = emptyScan(LIB);
    const ev = (id: string, created?: boolean) =>
      ({
        type: "scan.series_updated",
        library_id: LIB,
        series_id: id,
        name: `Series ${id}`,
        ...(created === undefined ? {} : { created, cover_url: `/c/${id}` }),
      }) as ScanEvent;
    s = applyScanEvent(s, ev("a", true), 0);
    s = applyScanEvent(s, ev("a", true), 1);
    s = applyScanEvent(s, ev("b", false), 2);
    s = applyScanEvent(s, ev("c"), 3);
    expect(s.discovered.map((d) => d.seriesId)).toEqual(["c", "b", "a"]);
    expect(s.discovered[2]).toMatchObject({ coverUrl: "/c/a", created: true });
    expect(s.counters.newSeries).toBe(1);
  });

  it("finishes with the result and logs it", () => {
    let s = applyScanEvent(
      emptyScan(LIB),
      progress({ completed: 50, total: 100 }),
      0,
    );
    s = applyScanEvent(
      s,
      {
        type: "scan.completed",
        library_id: LIB,
        scan_id: "s1",
        added: 3,
        updated: 1,
        removed: 0,
        duration_ms: 4200,
      } as ScanEvent,
      10,
    );
    expect(s.status).toBe("complete");
    expect(s.completed).toBe(100);
    expect(s.result).toMatchObject({ added: 3, durationMs: 4200 });
    expect(s.log.at(-1)).toMatchObject({ tone: "ok" });
  });

  it("seeds from the snapshot until live events flow", () => {
    const seeded = seedFromActive(emptyScan(LIB), {
      id: "s1",
      kind: "library",
      state: "running",
      started_at: "x",
      stalled: false,
      phase: "scanning",
      completed: 40,
      total: 80,
      current_label: "Saga",
      files_per_sec: 12,
    });
    expect(seeded).toMatchObject({ scanId: "s1", completed: 40, rate: 12 });
  });
});

describe("formatting", () => {
  it("humanizes health-issue kinds", () => {
    expect(humanizeKind("OrphanedSeriesJson")).toBe("orphaned series json");
    expect(humanizeKind("missing_file")).toBe("missing file");
  });

  it("does not log the generic scanning label as a folder", () => {
    const s = applyScanEvent(
      emptyScan(LIB),
      progress({ current_label: "Scanning files" }),
      0,
    );
    expect(s.label).toBeNull();
    expect(s.log.some((l) => /scanned/.test(l.text))).toBe(false);
  });

  it("formats ETAs", () => {
    expect(formatEta(null)).toBe("—");
    expect(formatEta(42_000)).toBe("42s");
    expect(formatEta(65_000)).toBe("1m 05s");
    expect(formatEta(3_780_000)).toBe("1h 03m");
  });

  it("draws a sparkline anchored to the right edge", () => {
    expect(sparkPath([1], 400, 84)).toBeNull();
    const p = sparkPath([1, 2, 3], 400, 84)!;
    expect(p.line.startsWith("M")).toBe(true);
    expect(p.line).toContain("400.0,");
    expect(p.area.endsWith("Z")).toBe(true);
  });
});
