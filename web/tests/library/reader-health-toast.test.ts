/**
 * The reader's one-time health toast speaks only for warning rows
 * (dropped pages) and info rows (a repaired archive). Error rows, such as
 * an archive refused by a safety limit, produce no toast: a reader learns
 * nothing about the cause, which stays on the admin health page.
 */
import { describe, expect, it } from "vitest";

import { describeHealthToast } from "@/app/[locale]/read/[seriesSlug]/[issueSlug]/ReaderHealthToast";
import type { HealthIssueView } from "@/lib/api/types";

function row(
  kind: HealthIssueView["kind"],
  severity: string,
  data: Record<string, unknown>,
): HealthIssueView {
  return {
    id: kind,
    kind,
    severity,
    fingerprint: kind,
    payload: { data },
    first_seen_at: "2026-11-10T00:00:00Z",
    last_seen_at: "2026-11-10T00:00:00Z",
    resolved_at: null,
    dismissed_at: null,
    scan_id: null,
  };
}

const refused = row("ArchiveSafetyRefusal", "error", {
  path: "/lib/x.cbz",
  check: "entry_count",
  error: "archive cap exceeded: entry count",
});

describe("describeHealthToast", () => {
  it("is silent for an error-only file", () => {
    expect(describeHealthToast([refused])).toBeNull();
    expect(
      describeHealthToast([
        row("MalformedArchive", "error", { path: "/lib/x.cbz", error: "e" }),
      ]),
    ).toBeNull();
  });

  it("never claims a refused file was repaired", () => {
    const out = describeHealthToast([
      refused,
      row("RecoveredArchive", "info", { technique: "cdfh-offset-rebuild" }),
    ]);
    expect(out?.severity).toBe("info");
    expect(out?.message).toContain("cdfh-offset-rebuild");
    expect(out?.message).not.toMatch(/limit|entry count|refused/i);
  });

  it("keeps the partial-file warning", () => {
    const out = describeHealthToast([
      row("SkippedArchiveEntries", "warning", {
        dropped: 2,
        total: 10,
        reason: "compression ratio cap",
      }),
    ]);
    expect(out?.severity).toBe("warning");
    expect(out?.message).toContain("2 of 10");
  });
});
