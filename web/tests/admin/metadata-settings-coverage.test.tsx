// @vitest-environment jsdom
/**
 * /admin/metadata → Settings: "Coverage after series matches"
 * (`metadata.coverage_after_series_apply` + `metadata.coverage_auto_accept`).
 * The form only PATCHes keys that changed; defaults are `manual_only` / off.
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

describe("Coverage after series matches settings", () => {
  it("shows the default and patches only the auto-accept switch", async () => {
    const onSubmit = vi.fn(async () => undefined);
    render(
      <SettingsForm initial={INITIAL} isPending={false} onSubmit={onSubmit} />,
    );
    expect(screen.getByText("Coverage after series matches")).toBeTruthy();
    const trigger = screen.getByRole("combobox", { name: "Run coverage" });
    expect(trigger.textContent).toContain("After matches you apply (default)");

    const save = screen.getByRole("button", { name: "Save" });
    expect((save as HTMLButtonElement).disabled).toBe(true);
    fireEvent.click(
      screen.getByRole("switch", {
        name: "Accept high-confidence results automatically",
      }),
    );
    expect((save as HTMLButtonElement).disabled).toBe(false);
    fireEvent.click(save);
    await waitFor(() =>
      expect(onSubmit).toHaveBeenCalledWith({
        "metadata.coverage_auto_accept": true,
      }),
    );
  });

  it("disables auto-accept when coverage is off", () => {
    render(
      <SettingsForm
        initial={{ ...INITIAL, coverageAfterApply: "off" }}
        isPending={false}
        onSubmit={async () => undefined}
      />,
    );
    expect(
      screen.getByRole("combobox", { name: "Run coverage" }).textContent,
    ).toContain("Off");
    const sw = screen.getByRole("switch", {
      name: "Accept high-confidence results automatically",
    }) as HTMLButtonElement;
    expect(sw.disabled).toBe(true);
  });
});
