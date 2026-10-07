import { describe, expect, it } from "vitest";

import {
  healthKindHint,
  healthKindLabel,
  healthPayloadDetails,
  healthPayloadRefs,
  healthPayloadSummary,
} from "@/lib/library/health-issues";

const SERIES_ID = "0190a4f2-0000-7000-8000-000000000001";

describe("archive-container kinds", () => {
  it("names a mislabeled container on UnsupportedArchiveFormat", () => {
    const payload = {
      kind: "unsupported_archive_format",
      data: { path: "/lib/MM (2006)/MM 002.cbz", ext: "cbr" },
    };
    expect(healthPayloadSummary("UnsupportedArchiveFormat", payload)).toBe(
      "/lib/MM (2006)/MM 002.cbz — CBR archive mislabeled as .cbz",
    );
    expect(healthKindHint("UnsupportedArchiveFormat")).toMatch(/conversion/);
  });

  it("keeps a plain summary when the extension matches the container", () => {
    const payload = {
      kind: "unsupported_archive_format",
      data: { path: "/lib/Thanos (2020)/Thanos 001.cbr", ext: "cbr" },
    };
    expect(healthPayloadSummary("UnsupportedArchiveFormat", payload)).toBe(
      "/lib/Thanos (2020)/Thanos 001.cbr — CBR archive",
    );
  });

  it("distinguishes a corrupt archive from a malformed ComicInfo", () => {
    expect(healthKindLabel("MalformedArchive")).toMatch(/archive/i);
    expect(healthKindLabel("MalformedArchive")).not.toMatch(/ComicInfo/);
    expect(healthKindHint("MalformedArchive")).toMatch(/Replace/);
    expect(
      healthPayloadSummary("MalformedArchive", {
        kind: "malformed_archive",
        data: { path: "/lib/X 001.cbz", error: "Could not find EOCD" },
      }),
    ).toBe("/lib/X 001.cbz — Could not find EOCD");
  });
});

describe("health-issue presentation (WP-3.4 kinds)", () => {
  it("summarizes FolderNameMismatch and links the series", () => {
    const payload = {
      kind: "folder_name_mismatch",
      data: {
        folder: "/lib/Batman (2016)",
        series_id: SERIES_ID,
        comic_info_series: "Batman: Rebirth",
        files: 2,
      },
    };
    expect(healthKindLabel("FolderNameMismatch")).toMatch(/Folder name/);
    expect(healthKindHint("FolderNameMismatch")).toBeTruthy();
    expect(healthPayloadSummary("FolderNameMismatch", payload)).toBe(
      "/lib/Batman (2016) — ComicInfo says “Batman: Rebirth” (2 files)",
    );
    expect(healthPayloadRefs(payload).seriesIds).toEqual([SERIES_ID]);
  });

  it("lists MixedSeriesInFolder values with counts and overflow", () => {
    const payload = {
      kind: "mixed_series_in_folder",
      data: {
        folder: "/lib/Saga",
        series_id: SERIES_ID,
        distinct_values: 12,
        series_values: [
          { series: "Saga", files: 2, example: "Saga 001.cbz" },
          { series: "Paper Girls", files: 1, example: "Paper Girls 001.cbz" },
        ],
      },
    };
    expect(healthPayloadSummary("MixedSeriesInFolder", payload)).toBe(
      "/lib/Saga — 12 different ComicInfo series",
    );
    const details = healthPayloadDetails("MixedSeriesInFolder", payload);
    expect(details?.lines).toEqual([
      "Saga — 2 files (e.g. Saga 001.cbz)",
      "Paper Girls — 1 file (e.g. Paper Girls 001.cbz)",
    ]);
    expect(details?.more).toBe(10);
  });

  it("previews the AmbiguousFolder skipped subtree", () => {
    const payload = {
      kind: "ambiguous_folder",
      data: {
        path: "/lib/DC/Vertigo",
        reason: "folder appears to be a third nesting level",
        skipped_archives: [
          "Preacher/Preacher 000.cbz",
          "Sandman/Sandman 000.cbz",
        ],
        skipped_archive_count: 28,
      },
    };
    expect(healthPayloadSummary("AmbiguousFolder", payload)).toBe(
      "/lib/DC/Vertigo — folder appears to be a third nesting level — 28 archives skipped",
    );
    const details = healthPayloadDetails("AmbiguousFolder", payload);
    expect(details?.title).toBe("Skipped archives");
    expect(details?.lines).toHaveLength(2);
    expect(details?.more).toBe(26);
  });

  it("tolerates pre-WP-3.4 AmbiguousFolder rows without a preview", () => {
    const payload = {
      kind: "ambiguous_folder",
      data: { path: "/lib/X", reason: "r" },
    };
    expect(healthPayloadSummary("AmbiguousFolder", payload)).toBe("/lib/X — r");
    expect(healthPayloadDetails("AmbiguousFolder", payload)).toBeNull();
  });

  it("summarizes OrphanedSeriesJson by folder", () => {
    expect(
      healthPayloadSummary("OrphanedSeriesJson", {
        kind: "orphaned_series_json",
        data: { folder: "/lib/Gone" },
      }),
    ).toBe("/lib/Gone");
  });

  it("unwraps nested payloads for generic kinds (Findings page bug)", () => {
    expect(
      healthPayloadSummary("UnreadableArchive", {
        kind: "unreadable_archive",
        data: { path: "/lib/a.cbz", error: "bad zip" },
      }),
    ).toBe("/lib/a.cbz — bad zip");
    expect(healthKindLabel("SomethingNew")).toBe("SomethingNew");
  });
});
