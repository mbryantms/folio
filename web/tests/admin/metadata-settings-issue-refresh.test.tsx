// @vitest-environment jsdom
/**
 * /admin/metadata → Settings: "Issue-level refresh"
 * (`metadata.issue_refresh_enabled` + `metadata.issue_refresh_per_provider_cap`).
 * Off by default with a 200-per-provider cap; the form only PATCHes keys
 * that changed and the cap input is disabled while the switch is off.
 */
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

vi.mock("@/lib/api/mutations", () => ({
  useUpdateSettings: () => ({
    mutateAsync: async () => ({}),
    isPending: false,
  }),
}));
vi.mock("@/lib/api/queries", () => ({
  useAdminSettings: () => ({ isLoading: false, data: { values: [] } }),
}));

import { SettingsForm } from "@/components/admin/metadata/SettingsTab";

const INITIAL = {
  enabled: false,
  cron: "0 0 4 * * 0",
  windowDays: 14,
  staleAfterDays: 180,
  autoApplyThreshold: 80,
  matchMediumThreshold: 60,
  coverageAfterApply: "manual_only" as const,
  coverageAutoAccept: false,
  issueRefreshEnabled: false,
  issueRefreshCap: 200,
};

describe("Issue-level refresh settings", () => {
  it("is off by default; turning it on and changing the cap patches both", async () => {
    const onSubmit = vi.fn(async () => undefined);
    render(
      <SettingsForm initial={INITIAL} isPending={false} onSubmit={onSubmit} />,
    );
    expect(screen.getByText("Issue-level refresh")).toBeTruthy();
    const sw = screen.getByRole("switch", { name: "Refresh issues too" });
    expect(sw.getAttribute("aria-checked")).toBe("false");
    const cap = screen.getByLabelText(
      "Issues per provider per run",
    ) as HTMLInputElement;
    expect(cap.value).toBe("200");
    expect(cap.disabled).toBe(true);

    fireEvent.click(sw);
    await waitFor(() => expect(cap.disabled).toBe(false));
    fireEvent.change(cap, { target: { value: "50" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() =>
      expect(onSubmit).toHaveBeenCalledWith({
        "metadata.issue_refresh_enabled": true,
        "metadata.issue_refresh_per_provider_cap": 50,
      }),
    );
  });

  it("never sends an out-of-range cap", () => {
    const onSubmit = vi.fn(async () => undefined);
    render(
      <SettingsForm
        initial={{ ...INITIAL, issueRefreshEnabled: true }}
        isPending={false}
        onSubmit={onSubmit}
      />,
    );
    const cap = screen.getByLabelText(
      "Issues per provider per run",
    ) as HTMLInputElement;
    fireEvent.change(cap, { target: { value: "5000" } });
    expect(cap.validity.rangeOverflow).toBe(true);
    // The form's constraint validation blocks the submit (synchronous).
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(onSubmit).not.toHaveBeenCalled();
  });
});
