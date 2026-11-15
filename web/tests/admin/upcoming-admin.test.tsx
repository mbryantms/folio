// @vitest-environment jsdom
/**
 * /admin/upcoming (upcoming-releases 1.0): the release calendar's
 * switches and state. The settings card PATCHes only the keys that
 * changed; the status card shows what the list pass left and its "Sync
 * now" is available only while the calendar can actually run.
 */
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { MylarStatusView, UpcomingStatusView } from "@/lib/api/types";

const mutateAsync = vi.fn(async () => ({}));
const sync = vi.fn();
const createToken = vi.fn(
  (_input: undefined, opts?: { onSuccess?: (d: { token: string }) => void }) =>
    opts?.onSuccess?.({ token: "fmi_freshlymintedtoken" }),
);
const revokeToken = vi.fn();
const state = vi.hoisted(() => ({
  settings: [] as { key: string; value: unknown }[],
  status: null as unknown,
}));

vi.mock("@/lib/api/mutations", () => ({
  useUpdateSettings: () => ({ mutateAsync, isPending: false }),
  useSyncUpcomingCalendar: () => ({ mutate: sync, isPending: false }),
  useCreateMylarToken: () => ({ mutate: createToken, isPending: false }),
  useRevokeMylarToken: () => ({ mutate: revokeToken, isPending: false }),
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

function mylar(over: Partial<MylarStatusView> = {}): MylarStatusView {
  return {
    enabled: false,
    stale_after_minutes: 180,
    token: null,
    snapshot: null,
    ...over,
  };
}

const TOKEN = {
  created_at: "2026-11-14T10:00:00Z",
  last_used_at: "2026-11-14T12:00:00Z",
  last_used_ip: "192.168.1.245",
};

const SNAPSHOT = {
  received_at: "2026-11-14T12:00:00Z",
  generated_at: "2026-11-14T11:59:58Z",
  freshness: "fresh" as const,
  pull_count: 168,
  watch_count: 4319,
  wanted_count: 13,
  queue_counts: { Downloading: 12, Failed: 546, Queued: 1 },
  annuals_included: false,
};

function status(over: Partial<UpcomingStatusView> = {}): UpcomingStatusView {
  return {
    enabled: true,
    metron_ready: true,
    weeks_ahead: 8,
    mylar: mylar(),
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
  createToken.mockClear();
  revokeToken.mockClear();
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

/** The Save button of the form that holds the field labelled `label`. */
function saveIn(label: string): HTMLButtonElement {
  const form = screen.getByLabelText(label).closest("form");
  expect(form).not.toBeNull();
  return Array.from(form!.querySelectorAll("button")).find(
    (b) => b.textContent === "Save",
  ) as HTMLButtonElement;
}

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
    const calendar = screen
      .getByText("Release calendar")
      .closest("[data-slot=card]")!;
    expect(calendar.textContent).toContain("OFF");
    expect(screen.getByText(/paused at the week of 23 Nov/)).toBeTruthy();
    expect(screen.getByText(/provider rejected credentials/)).toBeTruthy();
    expect(
      (screen.getByRole("button", { name: "Sync now" }) as HTMLButtonElement)
        .disabled,
    ).toBe(true);
  });

  it("saves only what changed and rejects an out-of-range week count", async () => {
    render(<UpcomingAdmin />);
    const save = saveIn("Weeks ahead");
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
    fireEvent.click(saveIn("Weeks ahead"));
    await waitFor(() => expect(mutateAsync).toHaveBeenCalledTimes(1));
    expect(mutateAsync).toHaveBeenCalledWith({ "upcoming.enabled": true });
  });
});

describe("/admin/upcoming — Mylar card", () => {
  it("is OFF by default and says it is view-only", () => {
    render(<UpcomingAdmin />);
    const card = screen.getByText("Mylar").closest("[data-slot=card]")!;
    expect(card.textContent).toContain("OFF");
    expect(card.textContent).toContain("Folio never");
    expect(card.textContent).toContain("Nothing received yet");
    expect(screen.getByRole("button", { name: "Generate token" })).toBeTruthy();
  });

  it("walks the states: no token, waiting, receiving, stale", () => {
    state.status = status({ mylar: mylar({ enabled: true }) });
    const { rerender } = render(<UpcomingAdmin />);
    expect(screen.getByText("NO TOKEN")).toBeTruthy();

    state.status = status({ mylar: mylar({ enabled: true, token: TOKEN }) });
    rerender(<UpcomingAdmin />);
    expect(screen.getByText("WAITING FOR A SNAPSHOT")).toBeTruthy();
    expect(screen.getByText(/from 192\.168\.1\.245/)).toBeTruthy();

    state.status = status({
      mylar: mylar({ enabled: true, token: TOKEN, snapshot: SNAPSHOT }),
    });
    rerender(<UpcomingAdmin />);
    expect(screen.getByText("RECEIVING")).toBeTruthy();
    expect(screen.getByText("168 lines")).toBeTruthy();
    expect(screen.getByText("4,319 series")).toBeTruthy();
    expect(
      screen.getByText("12 downloading · 546 failed · 1 queued"),
    ).toBeTruthy();

    state.status = status({
      mylar: mylar({
        enabled: true,
        token: TOKEN,
        snapshot: { ...SNAPSHOT, freshness: "stale" },
      }),
    });
    rerender(<UpcomingAdmin />);
    expect(screen.getByText("STALE")).toBeTruthy();
    expect(screen.getByText("The snapshot is stale")).toBeTruthy();
    expect(
      screen.getByText("systemctl status folio-mylar-push.service"),
    ).toBeTruthy();
  });

  it("warns when Mylar is on but upcoming releases is off", () => {
    state.settings = [{ key: "upcoming.enabled", value: false }];
    state.status = status({
      enabled: false,
      mylar: mylar({ enabled: true, token: TOKEN }),
    });
    render(<UpcomingAdmin />);
    expect(screen.getByText("Upcoming releases is off")).toBeTruthy();
  });

  it("generates a token and shows it once", () => {
    state.status = status({ mylar: mylar({ enabled: true }) });
    render(<UpcomingAdmin />);
    fireEvent.click(screen.getByRole("button", { name: "Generate token" }));
    expect(createToken).toHaveBeenCalledTimes(1);
    expect(screen.getByTestId("mylar-token").textContent).toBe(
      "fmi_freshlymintedtoken",
    );
    expect(screen.getByText(/shown this once/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Done" }));
    expect(screen.queryByTestId("mylar-token")).toBeNull();
  });

  it("confirms before replacing a token", () => {
    state.status = status({ mylar: mylar({ enabled: true, token: TOKEN }) });
    render(<UpcomingAdmin />);
    fireEvent.click(screen.getByRole("button", { name: "Replace token" }));
    expect(createToken).not.toHaveBeenCalled();
    expect(screen.getByText("Replace the ingest token?")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Replace" }));
    expect(createToken).toHaveBeenCalledTimes(1);
  });

  it("revokes with or without deleting the snapshot", () => {
    state.status = status({
      mylar: mylar({ enabled: true, token: TOKEN, snapshot: SNAPSHOT }),
    });
    render(<UpcomingAdmin />);
    fireEvent.click(screen.getByRole("button", { name: "Revoke" }));
    expect(screen.getByText("Revoke the ingest token?")).toBeTruthy();
    fireEvent.click(screen.getByLabelText("Also delete the stored snapshot"));
    const confirm = screen
      .getAllByRole("button", { name: "Revoke" })
      .find((b) => b.closest("[role=alertdialog]"))!;
    fireEvent.click(confirm);
    expect(revokeToken).toHaveBeenCalledWith({ purge: true });
  });

  it("saves only the Mylar keys that changed", async () => {
    render(<UpcomingAdmin />);
    const save = saveIn("Show as stale after (minutes)");
    expect(save.disabled).toBe(true);
    const stale = screen.getByLabelText(
      "Show as stale after (minutes)",
    ) as HTMLInputElement;
    fireEvent.change(stale, { target: { value: "10" } });
    expect(save.disabled).toBe(true);
    fireEvent.change(stale, { target: { value: "180" } });
    fireEvent.click(screen.getByLabelText("Accept Mylar snapshots"));
    expect(save.disabled).toBe(false);
    fireEvent.click(save);
    await waitFor(() => expect(mutateAsync).toHaveBeenCalledTimes(1));
    expect(mutateAsync).toHaveBeenCalledWith({
      "integrations.mylar.enabled": true,
    });
  });
});
