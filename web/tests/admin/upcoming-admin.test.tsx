// @vitest-environment jsdom
/**
 * /admin/upcoming (upcoming-releases 1.0): the release calendar's
 * switches and state. The settings card PATCHes only the keys that
 * changed; the status card shows what the list pass left and its "Sync
 * now" is available only while the calendar can actually run.
 */
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { UpcomingStatusView } from "@/lib/api/types";

const mutateAsync = vi.fn(async () => ({}));
const sync = vi.fn();
const state = vi.hoisted(() => ({
  settings: [] as { key: string; value: unknown }[],
  status: null as unknown,
}));

vi.mock("@/lib/api/mutations", () => ({
  useUpdateSettings: () => ({ mutateAsync, isPending: false }),
  useSyncUpcomingCalendar: () => ({ mutate: sync, isPending: false }),
}));
vi.mock("@/lib/api/queries", () => ({
  useAdminSettings: () => ({
    isLoading: false,
    data: { values: state.settings },
  }),
  useAdminUpcomingStatus: () => ({ isLoading: false, data: state.status }),
}));

import {
  UpcomingAdmin,
  shortDay,
} from "@/components/admin/upcoming/UpcomingAdmin";

function status(over: Partial<UpcomingStatusView> = {}): UpcomingStatusView {
  return {
    enabled: true,
    metron_ready: true,
    weeks_ahead: 8,
    calendar: {
      window_start: "2026-10-26",
      window_end: "2027-01-10",
      issues: 163,
      series: 120,
      series_pending_detail: 14,
      series_without_comicvine: 3,
      weeks: [
        { week_start: "2026-11-09", issues: 84 },
        { week_start: "2026-11-16", issues: 79 },
      ],
      last_finished_at: null,
      resumes_at_week: null,
      last_error: null,
      last_error_at: null,
      next_attempt_at: null,
    },
    ...over,
  };
}

beforeEach(() => {
  mutateAsync.mockClear();
  sync.mockClear();
  state.settings = [
    { key: "upcoming.enabled", value: true },
    { key: "upcoming.weeks_ahead", value: 8 },
  ];
  state.status = status();
});

describe("shortDay", () => {
  it("formats a date with fixed wording", () => {
    expect(shortDay("2026-11-09")).toBe("9 Nov");
    expect(shortDay("2027-01-10")).toBe("10 Jan");
    expect(shortDay("nonsense")).toBe("nonsense");
  });
});

describe("/admin/upcoming", () => {
  it("shows the calendar's counts, per-week bars and pending details", () => {
    render(<UpcomingAdmin />);
    expect(screen.getByText("ON")).toBeTruthy();
    expect(screen.getByText("163")).toBeTruthy();
    expect(screen.getByText("No full sync yet.")).toBeTruthy();
    expect(screen.getByText(/14 series still waiting/)).toBeTruthy();
    const bars = screen.getByRole("list", { name: "Issues per week" });
    expect(bars.querySelectorAll("li")).toHaveLength(2);
    expect(
      screen.getByRole("progressbar", { name: /9 Nov: 84 issues/ }),
    ).toBeTruthy();
  });

  it("queues a sync when the calendar can run", () => {
    render(<UpcomingAdmin />);
    const button = screen.getByRole("button", { name: "Sync now" });
    expect((button as HTMLButtonElement).disabled).toBe(false);
    fireEvent.click(button);
    expect(sync).toHaveBeenCalledTimes(1);
  });

  it("disables Sync now and says why when Metron is not ready", () => {
    state.status = status({ metron_ready: false });
    render(<UpcomingAdmin />);
    expect(screen.getByText("METRON NOT READY")).toBeTruthy();
    expect(
      screen.getByText(/Metron is off or has no credentials/),
    ).toBeTruthy();
    expect(
      (screen.getByRole("button", { name: "Sync now" }) as HTMLButtonElement)
        .disabled,
    ).toBe(true);
  });

  it("shows OFF and a paused or failed sync", () => {
    state.settings = [{ key: "upcoming.enabled", value: false }];
    state.status = status({
      enabled: false,
      calendar: {
        ...status().calendar,
        resumes_at_week: "2026-11-23",
        last_error: "provider rejected credentials: nope",
        last_error_at: "2026-11-14T10:00:00Z",
        next_attempt_at: "2026-11-14T10:30:00Z",
      },
    });
    render(<UpcomingAdmin />);
    expect(screen.getByText("OFF")).toBeTruthy();
    expect(screen.getByText(/paused at the week of 23 Nov/)).toBeTruthy();
    expect(screen.getByText(/provider rejected credentials/)).toBeTruthy();
    expect(
      (screen.getByRole("button", { name: "Sync now" }) as HTMLButtonElement)
        .disabled,
    ).toBe(true);
  });

  it("saves only what changed and rejects an out-of-range week count", async () => {
    render(<UpcomingAdmin />);
    const save = screen.getByRole("button", {
      name: "Save",
    }) as HTMLButtonElement;
    expect(save.disabled).toBe(true);

    const weeks = screen.getByLabelText("Weeks ahead") as HTMLInputElement;
    expect(weeks.value).toBe("8");
    fireEvent.change(weeks, { target: { value: "13" } });
    expect(save.disabled).toBe(true);
    fireEvent.change(weeks, { target: { value: "6" } });
    expect(save.disabled).toBe(false);

    fireEvent.click(save);
    await waitFor(() => expect(mutateAsync).toHaveBeenCalledTimes(1));
    expect(mutateAsync).toHaveBeenCalledWith({ "upcoming.weeks_ahead": 6 });
  });

  it("turns the feature on from the default-off state", async () => {
    state.settings = [];
    state.status = status({ enabled: false });
    render(<UpcomingAdmin />);
    fireEvent.click(screen.getByLabelText("Show upcoming releases"));
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(mutateAsync).toHaveBeenCalledTimes(1));
    expect(mutateAsync).toHaveBeenCalledWith({ "upcoming.enabled": true });
  });
});
