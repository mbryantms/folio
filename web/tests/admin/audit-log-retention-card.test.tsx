// @vitest-environment jsdom
/**
 * /admin/server → "Audit log retention" (`audit_log.retention_days`).
 * 0 keeps every row (default); any other value must be at least 90 days.
 * The card PATCHes only its own key.
 */
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

const mutateAsync = vi.fn(async () => ({}));
vi.mock("@/lib/api/mutations", () => ({
  useUpdateSettings: () => ({ mutateAsync, isPending: false }),
}));
vi.mock("@/lib/api/queries", () => ({
  useAdminSettings: () => ({
    isLoading: false,
    data: { values: [{ key: "audit_log.retention_days", value: 0 }] },
  }),
}));

import {
  ServerSettingsCards,
  auditLogRetentionProblem,
} from "@/components/admin/server/ServerSettingsCards";

describe("auditLogRetentionProblem", () => {
  it("accepts 0 and values from the 90-day floor up", () => {
    expect(auditLogRetentionProblem("0")).toBeNull();
    expect(auditLogRetentionProblem("90")).toBeNull();
    expect(auditLogRetentionProblem("365")).toBeNull();
    expect(auditLogRetentionProblem("36500")).toBeNull();
  });

  it("rejects non-numbers, below-floor and absurd values", () => {
    expect(auditLogRetentionProblem("")).toMatch(/whole number/);
    expect(auditLogRetentionProblem("-1")).toMatch(/whole number/);
    expect(auditLogRetentionProblem("12.5")).toMatch(/whole number/);
    expect(auditLogRetentionProblem("1")).toMatch(
      /Must be 0 or in \[90, 36500\]/,
    );
    expect(auditLogRetentionProblem("89")).toMatch(/Must be 0 or in/);
    expect(auditLogRetentionProblem("36501")).toMatch(/Must be 0 or in/);
  });
});

describe("Audit log retention card", () => {
  it("shows the keep-forever default, blocks a below-floor value and saves a valid one", async () => {
    render(<ServerSettingsCards />);
    expect(screen.getByText("Audit log retention")).toBeTruthy();
    const input = screen.getByLabelText(
      "Keep audit entries for (days, 0 = forever)",
    ) as HTMLInputElement;
    expect(input.value).toBe("0");

    const card = input.closest("[data-slot=card]");
    expect(card).not.toBeNull();
    const save = Array.from(card!.querySelectorAll("button")).find(
      (b) => b.textContent === "Save",
    ) as HTMLButtonElement;
    expect(save.disabled).toBe(true);

    fireEvent.change(input, { target: { value: "30" } });
    expect(screen.getByText("Must be 0 or in [90, 36500].")).toBeTruthy();
    expect(save.disabled).toBe(true);

    fireEvent.change(input, { target: { value: "180" } });
    expect(screen.queryByText("Must be 0 or in [90, 36500].")).toBeNull();
    expect(save.disabled).toBe(false);
    fireEvent.click(save);
    await waitFor(() =>
      expect(mutateAsync).toHaveBeenCalledWith({
        "audit_log.retention_days": 180,
      }),
    );
  });
});
