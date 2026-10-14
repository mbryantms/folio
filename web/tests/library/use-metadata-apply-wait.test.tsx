// @vitest-environment jsdom
/**
 * Post-audit T1.6: the post-apply wait is correlated to *this* apply.
 * An unrelated completion in the same library no longer resolves it, a
 * failed job stops it with the error, and an overdue wait stays open
 * (D5a) instead of timing out into a false success.
 */
import { act, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { ScanEvent } from "@/lib/api/types";

const ws = vi.hoisted(() => ({
  events: [] as ScanEvent[],
  setEvents: null as null | ((e: ScanEvent[]) => void),
}));
vi.mock("@/lib/api/scan-events", async () => {
  const React = await import("react");
  return {
    useScanEvents: () => {
      const [events, setEvents] = React.useState<ScanEvent[]>(ws.events);
      ws.setEvents = setEvents;
      return { status: "open" as const, events };
    },
  };
});

import {
  APPLY_WAIT_OVERDUE_MS,
  eventCompletesApply,
  type ApplyTarget,
  useMetadataApplyWait,
} from "@/components/library/useMetadataApplyWait";

const LIB = "lib-1";
const target: ApplyTarget = {
  runId: "run-1",
  seriesId: "ser-1",
  issueId: null,
  settled: false,
};

function completed(
  extra: Partial<Extract<ScanEvent, { type: "scan.completed" }>> = {},
): ScanEvent {
  return {
    type: "scan.completed",
    library_id: LIB,
    scan_id: "scan-1",
    added: 0,
    updated: 1,
    removed: 0,
    duration_ms: 10,
    ...extra,
  };
}

function mount(t: ApplyTarget | null = target) {
  const rehydrate = vi.fn();
  const onApplied = vi.fn();
  const onClose = vi.fn();
  const hook = renderHook(
    ({ tgt }: { tgt: ApplyTarget | null }) =>
      useMetadataApplyWait({
        target: tgt,
        libraryId: LIB,
        isSeriesScope: true,
        rehydrate,
        onApplied,
        onClose,
      }),
    { initialProps: { tgt: t } },
  );
  return { ...hook, rehydrate, onApplied, onClose };
}

function push(events: ScanEvent[]) {
  act(() => {
    ws.setEvents?.(events);
  });
}

afterEach(() => {
  vi.useRealTimers();
  ws.events = [];
  ws.setEvents = null;
});

describe("eventCompletesApply", () => {
  it("matches metadata.applied on run id, and on entity ids for old servers", () => {
    const base = {
      type: "metadata.applied" as const,
      library_id: LIB,
      series_id: "ser-1",
    };
    expect(eventCompletesApply({ ...base, run_id: "run-1" }, target)).toBe(
      true,
    );
    expect(eventCompletesApply({ ...base, run_id: "run-2" }, target)).toBe(
      false,
    );
    expect(eventCompletesApply(base, target)).toBe(true);
    expect(eventCompletesApply({ ...base, series_id: "ser-9" }, target)).toBe(
      false,
    );
    // Issue apply: the issue must match.
    const issueTarget = { ...target, issueId: "iss-1" };
    expect(
      eventCompletesApply({ ...base, issue_id: "iss-1" }, issueTarget),
    ).toBe(true);
    expect(
      eventCompletesApply({ ...base, issue_id: "iss-2" }, issueTarget),
    ).toBe(false);
  });

  it("matches scan.completed only on its scope", () => {
    expect(eventCompletesApply(completed(), target)).toBe(false); // library-wide
    expect(eventCompletesApply(completed({ series_id: "ser-9" }), target)).toBe(
      false,
    );
    expect(eventCompletesApply(completed({ series_id: "ser-1" }), target)).toBe(
      true,
    );
    // An issue rescan inside the series is not the series apply's rescan.
    expect(
      eventCompletesApply(
        completed({ series_id: "ser-1", issue_id: "iss-1" }),
        target,
      ),
    ).toBe(false);
    expect(
      eventCompletesApply(
        completed({ series_id: "ser-1", issue_id: "iss-1" }),
        {
          ...target,
          issueId: "iss-1",
        },
      ),
    ).toBe(true);
  });
});

describe("useMetadataApplyWait", () => {
  it("ignores an unrelated completion and resolves on the matching one", () => {
    const h = mount();
    expect(h.result.current.waitingForRescan).toBe(true);
    push([completed({ scan_id: "other", series_id: "ser-9" })]);
    expect(h.result.current.waitingForRescan).toBe(true);
    expect(h.onApplied).not.toHaveBeenCalled();
    push([
      completed({ scan_id: "other", series_id: "ser-9" }),
      completed({ series_id: "ser-1" }),
    ]);
    expect(h.result.current.waitingForRescan).toBe(false);
    expect(h.rehydrate).toHaveBeenCalledTimes(1);
    expect(h.onApplied).toHaveBeenCalledTimes(1);
    expect(h.onClose).not.toHaveBeenCalled();
  });

  it("stops with the error on metadata.failed for the run", () => {
    const h = mount();
    push([
      {
        type: "metadata.failed",
        library_id: LIB,
        run_id: "run-1",
        series_id: "ser-1",
        error: "provider 502",
      },
    ]);
    expect(h.result.current.waitingForRescan).toBe(false);
    expect(h.result.current.failure).toBe("provider 502");
    expect(h.onApplied).not.toHaveBeenCalled();
    expect(h.onClose).not.toHaveBeenCalled();
  });

  it("an overdue wait stays open and keeps listening (D5a)", () => {
    vi.useFakeTimers();
    const h = mount();
    act(() => {
      vi.advanceTimersByTime(APPLY_WAIT_OVERDUE_MS + 1);
    });
    expect(h.result.current.waitingForRescan).toBe(true);
    expect(h.result.current.overdue).toBe(true);
    expect(h.onApplied).not.toHaveBeenCalled();
    // The late completion still resolves it.
    push([completed({ series_id: "ser-1" })]);
    expect(h.result.current.waitingForRescan).toBe(false);
    expect(h.onApplied).toHaveBeenCalledTimes(1);
  });

  it("a settled (synchronous) apply resolves at once", () => {
    const h = mount({ ...target, settled: true });
    expect(h.result.current.waitingForRescan).toBe(false);
    expect(h.rehydrate).toHaveBeenCalledTimes(1);
    expect(h.onApplied).toHaveBeenCalledTimes(1);
  });

  it("does nothing before an apply target exists", () => {
    const h = mount(null);
    expect(h.result.current.waitingForRescan).toBe(false);
    expect(h.onApplied).not.toHaveBeenCalled();
  });
});
