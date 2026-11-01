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

vi.mock("@/lib/api/mutations", () => ({
  useCancelMetadataBatch: () => ({ mutate: vi.fn(), isPending: false }),
  useCancelScanRun: () => ({ mutate: vi.fn(), isPending: false }),
}));
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
      thumbs_outstanding: 0,
      thumbs_per_min: null,
      thumbs_eta_secs: null,
      busy: false,
    },
    libraries: [],
    queues: [],
    metadata_batches: [],
    tasks: [],
    sidecar_reemits: [],
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
        thumbs_outstanding: 16_204,
        thumbs_per_min: 405,
        thumbs_eta_secs: 2_400,
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
      tasks: [
        {
          kind: "deep_validate",
          library_id: "a",
          started_at: "2026-10-04T23:48:00Z",
          scan_run_id: "run-dv",
          stalled: false,
        },
      ],
    });
    const html = render();

    expect(html).toContain("Work in progress");
    // A spawned task with no queue of its own.
    expect(html).toContain("Deep validation — Marvel");
    expect(html).toContain("started 12 min ago");
    expect(html).toContain("405/min · about 40 min left");
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
    // Every unfinished batch can be cancelled from here — the pill's trash
    // only clears scans and thumbnails.
    expect(html).toContain('aria-label="Cancel this metadata batch"');
    expect(html).toContain("10 / 40");
  });

  it("shows a library-wide metadata file rewrite with its counts", () => {
    state.data = view({
      sidecar_reemits: [
        {
          library_name: "Marvel",
          run: {
            run_id: "r1",
            library_id: "a",
            status: "running",
            started_at: "2026-10-05T00:00:00Z",
            finished_at: null,
            total: 1_000,
            enqueued: 300,
            skipped_refused: 12,
            skipped_gone: 0,
            rewritten: 250,
            failed: 3,
            outstanding: 47,
            stalled: false,
            note: null,
          },
        },
      ],
    });
    const html = render();
    expect(html).toContain("Metadata file rewrites");
    expect(html).toContain("Marvel");
    expect(html).toContain("queueing rewrites");
    // rewritten + failed + skipped of the library's total.
    expect(html).toContain("265 / 1,000");
    expect(html).toContain("12 skipped (format)");
    expect(html).toContain("3 failed");
    expect(html).not.toContain("Nothing queued.");
  });

  it("lists a stalled deep validation with a cancel control", () => {
    state.data = view({
      libraries: [
        {
          id: "a",
          slug: "marvel",
          name: "Marvel",
          scan: null,
          scoped_scans: 0,
          issues_total: 10,
          covers_ready: 10,
          covers_remaining: 0,
          covers_hash_only: 0,
          covers_errored: 0,
          cover_jobs_queued: 0,
          cover_jobs_running: 0,
          page_jobs_queued: 0,
          page_jobs_running: 0,
          hash_pending: 0,
          busy: false,
        },
      ],
      tasks: [
        {
          kind: "deep_validate",
          library_id: "a",
          started_at: "2026-10-04T22:00:00Z",
          scan_run_id: "run-dv",
          stalled: true,
        },
      ],
    });
    const html = render();
    // Its own label, not a scan.
    expect(html).toContain("Deep validation — Marvel");
    expect(html).toContain("stalled — nothing is running it");
    // Cancellable from here: a newer scan hides it from the live scan page.
    expect(html).toContain('aria-label="Cancel Deep validation — Marvel"');
    // A stalled task is not work in progress: no spinner.
    expect(html).not.toContain("animate-spin");
    expect(html).not.toContain("Nothing queued.");
  });

  it("says so when nothing is running", () => {
    state.data = view({});
    const html = render();
    expect(html).toContain("Idle — nothing is running or queued");
    expect(html).toContain("No libraries yet.");
    expect(html).toContain("Nothing queued.");
  });
});
