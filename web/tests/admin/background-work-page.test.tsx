/**
 * `<BackgroundWorkClient>` — the one page that shows everything in flight.
 *
 * Static-markup render with the snapshot hook mocked: a busy server shows
 * each library's scan + cover state and links onward; an idle one says so.
 */
import { describe, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { createElement } from "react";

import type { BackgroundWorkView } from "@/lib/api/types";

const state: { data: BackgroundWorkView | undefined } = { data: undefined };

vi.mock("@/lib/api/queries", () => ({
  useBackgroundWork: () => ({
    data: state.data,
    isLoading: false,
    isError: false,
  }),
}));

import { BackgroundWorkClient } from "@/components/admin/background/BackgroundWorkClient";

const queue = (name: string, waiting = 0, in_flight = 0) => ({
  queue: name,
  waiting,
  scheduled: 0,
  in_flight,
  dead: 0,
});

function view(over: Partial<BackgroundWorkView>): BackgroundWorkView {
  return {
    generated_at: "2026-10-05T00:00:00Z",
    totals: {
      scans_running: 0,
      scans_queued: 0,
      scans_stalled: 0,
      covers_remaining: 0,
      hash_pending: 0,
      jobs_outstanding: 0,
      jobs_in_flight: 0,
      jobs_dead: 0,
      busy: false,
    },
    libraries: [],
    queues: [],
    metadata_batches: [],
    ...over,
  };
}

const render = () => renderToStaticMarkup(createElement(BackgroundWorkClient));

describe("BackgroundWorkClient", () => {
  it("shows per-library scan and cover state, other queues and batches", () => {
    state.data = view({
      totals: {
        scans_running: 1,
        scans_queued: 1,
        scans_stalled: 0,
        covers_remaining: 16_204,
        hash_pending: 0,
        jobs_outstanding: 16_300,
        jobs_in_flight: 8,
        jobs_dead: 2,
        busy: true,
      },
      libraries: [
        {
          id: "a",
          slug: "marvel",
          name: "Marvel",
          scan: {
            id: "run-1",
            state: "running",
            kind: "library",
            started_at: "2026-10-05T00:00:00Z",
            batch_id: "batch-1",
            phase: "scanning",
            completed: 120,
            total: 480,
            current_label: "Fantastic Four",
            files_per_sec: 40,
            stalled: false,
          },
          scoped_scans: 0,
          issues_total: 19_626,
          covers_ready: 3_422,
          covers_remaining: 16_204,
          covers_hash_only: 0,
          covers_errored: 6,
          cover_jobs_queued: 16_196,
          cover_jobs_running: 8,
          page_jobs_queued: 0,
          page_jobs_running: 0,
          hash_pending: 0,
          busy: true,
        },
        {
          id: "b",
          slug: "image",
          name: "Image",
          scan: {
            id: "run-2",
            state: "queued",
            kind: "library",
            started_at: "2026-10-05T00:00:00Z",
            batch_id: "batch-1",
            phase: null,
            completed: null,
            total: null,
            current_label: null,
            files_per_sec: null,
            stalled: false,
          },
          scoped_scans: 0,
          issues_total: 2_281,
          covers_ready: 2_281,
          covers_remaining: 0,
          covers_hash_only: 0,
          covers_errored: 0,
          cover_jobs_queued: 0,
          cover_jobs_running: 0,
          page_jobs_queued: 0,
          page_jobs_running: 0,
          hash_pending: 0,
          busy: true,
        },
      ],
      queues: [
        queue("scan", 1, 1),
        queue("post_scan_thumbs", 16_196, 8),
        queue("rewrite_issue_sidecars", 90, 2),
      ],
      metadata_batches: [
        {
          id: "mb",
          library_id: null,
          scope: "library_refresh",
          status: "awaiting_quota",
          items_total: 40,
          items_finished: 10,
          created_at: "2026-10-05T00:00:00Z",
          stalled: false,
        },
      ],
    });
    const html = render();

    expect(html).toContain("Work in progress");
    // Each library row links to that library's live scan page.
    expect(html).toContain('href="/admin/libraries/marvel/scan"');
    expect(html).toContain('href="/admin/libraries/image/scan"');
    // Running scan: phase, progress and what it is on.
    expect(html).toContain("Scanning files");
    expect(html).toContain("120 / 480");
    expect(html).toContain("Fantastic Four");
    // Queued scan from the same scan-all.
    expect(html).toContain("Waiting for a scan worker");
    // Cover progress + job split.
    expect(html).toContain("3,422 / 19,626");
    expect(html).toContain("8 running · 16,196 queued");
    expect(html).toContain("6 errored");
    expect(html).toContain("Covers ready");
    // Queues with no library column, and the failed-jobs deep link.
    expect(html).toContain("Sidecar rewrite");
    expect(html).toContain("2 with workers");
    expect(html).toContain('href="/admin/queue?tab=failed"');
    // Metadata batch.
    expect(html).toContain("Library refresh");
    expect(html).toContain("waiting on provider quota");
    expect(html).toContain("10 / 40");
  });

  it("says so when nothing is running", () => {
    state.data = view({});
    const html = render();
    expect(html).toContain("Idle — nothing is running or queued");
    expect(html).toContain("No libraries yet.");
    expect(html).toContain("Nothing queued.");
  });
});
