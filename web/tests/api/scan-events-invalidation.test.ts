/**
 * Table-driven coverage for `invalidationsForEvent` (audit G5, risk #3).
 *
 * The scan-events WS routes each event to a set of query-key prefixes to
 * invalidate (coalesced). This pins every `ScanEvent` variant → its
 * expected scopes, so a routing change is a visible diff and a new event
 * type can't silently land with no mapping (the source switch is also
 * exhaustively typed, which fails the build on an unmapped variant).
 */
import { describe, expect, it } from "vitest";

import { invalidationsForEvent } from "@/lib/api/scan-events";
import { queryKeys } from "@/lib/api/queries";
import type { ScanEvent } from "@/lib/api/types";

const LIB = "lib-1";
const SCAN = "scan-1";

// One representative event per `ScanEvent.type`, paired with the exact
// key-prefix list it should enqueue.
const CASES: { evt: ScanEvent; expected: readonly (readonly unknown[])[] }[] = [
  {
    evt: { type: "scan.started", library_id: LIB, scan_id: SCAN, at: "t" },
    expected: [queryKeys.scanRunsAll(LIB)],
  },
  {
    evt: {
      type: "scan.completed",
      library_id: LIB,
      scan_id: SCAN,
      added: 1,
      updated: 2,
      removed: 0,
      duration_ms: 10,
    },
    expected: [
      queryKeys.scanRunsAll(LIB),
      queryKeys.library(LIB),
      queryKeys.health(LIB),
      queryKeys.removed(LIB),
      ["series"],
    ],
  },
  {
    evt: { type: "scan.failed", library_id: LIB, scan_id: SCAN, error: "x" },
    expected: [queryKeys.scanRunsAll(LIB)],
  },
  {
    evt: {
      type: "scan.health_issue",
      library_id: LIB,
      scan_id: SCAN,
      kind: "FileAtRoot",
      severity: "warning",
      path: null,
    },
    expected: [queryKeys.health(LIB)],
  },
  {
    evt: {
      type: "scan.series_updated",
      library_id: LIB,
      series_id: "ser-1",
      name: "S",
    },
    expected: [["series"]],
  },
  {
    evt: { type: "thumbs.started", library_id: LIB, issue_id: "iss-1" },
    expected: [queryKeys.thumbnailsStatus(LIB), queryKeys.queueDepth],
  },
  {
    evt: {
      type: "thumbs.completed",
      library_id: LIB,
      issue_id: "iss-1",
      pages: 3,
    },
    expected: [queryKeys.thumbnailsStatus(LIB), queryKeys.queueDepth],
  },
  {
    evt: {
      type: "thumbs.failed",
      library_id: LIB,
      issue_id: "iss-1",
      error: "x",
    },
    expected: [queryKeys.thumbnailsStatus(LIB), queryKeys.queueDepth],
  },
  {
    evt: { type: "metadata.applied", library_id: LIB, series_id: "ser-1" },
    expected: [
      ["issues"],
      ["series"],
      queryKeys.adminMetadataDashboard,
      queryKeys.adminMetadataMatchQuality,
      ["admin", "metadata", "recent-applies"],
    ],
  },
  {
    // Live progress is consumed from the events buffer — no invalidation.
    evt: {
      type: "scan.progress",
      library_id: LIB,
      scan_id: SCAN,
      kind: "library",
      phase: "scanning",
      unit: "files",
      completed: 1,
      total: 2,
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
    } as ScanEvent,
    expected: [],
  },
  {
    // Backfill drain finished → refresh queue depth + metadata dashboard.
    evt: {
      type: "backfill.completed",
      kind: "cover_phash",
      processed: 12,
      skipped: 1,
    },
    expected: [queryKeys.queueDepth, queryKeys.adminMetadataDashboard],
  },
  {
    // Dropped events → broad recovery sweep over every WS-driven cache.
    evt: { type: "lagged", skipped: 5 },
    expected: [["libraries"], ["admin"], ["series"], ["issues"]],
  },
];

describe("invalidationsForEvent", () => {
  for (const { evt, expected } of CASES) {
    it(`maps ${evt.type} to its expected key scopes`, () => {
      expect(invalidationsForEvent(evt)).toEqual(expected);
    });
  }

  it("covers every ScanEvent variant (no unmapped type)", () => {
    const covered = new Set(CASES.map((c) => c.evt.type));
    // Keep in lockstep with the ScanEvent union; the source switch is
    // exhaustively typed, this is the human-readable companion.
    const allTypes: ScanEvent["type"][] = [
      "scan.started",
      "scan.progress",
      "scan.series_updated",
      "scan.health_issue",
      "scan.completed",
      "scan.failed",
      "thumbs.started",
      "thumbs.completed",
      "thumbs.failed",
      "metadata.applied",
      "lagged",
    ];
    for (const t of allTypes) expect(covered.has(t)).toBe(true);
  });
});
