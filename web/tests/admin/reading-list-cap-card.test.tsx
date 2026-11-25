// @vitest-environment jsdom
/**
 * /admin/server → "Reading lists" (`cbl.max_remote_lists_per_user`).
 * Default 500, valid range [1, 10000]. The card PATCHes only its own key.
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
    data: { values: [] },
  }),
}));

import {
  ServerSettingsCards,
  remoteListCapProblem,
} from "@/components/admin/server/ServerSettingsCards";

describe("remoteListCapProblem", () => {
  it("accepts whole numbers in [1, 10000]", () => {
    expect(remoteListCapProblem("1")).toBeNull();
    expect(remoteListCapProblem("500")).toBeNull();
    expect(remoteListCapProblem("10000")).toBeNull();
  });

  it("rejects non-numbers and out-of-range values", () => {
    expect(remoteListCapProblem("")).toMatch(/whole number/);
    expect(remoteListCapProblem("-5")).toMatch(/whole number/);
    expect(remoteListCapProblem("2.5")).toMatch(/whole number/);
    expect(remoteListCapProblem("0")).toMatch(/Must be in \[1, 10000\]/);
    expect(remoteListCapProblem("10001")).toMatch(/Must be in/);
  });
});

describe("Reading lists card", () => {
  it("shows the 500 default, blocks an invalid value and saves a valid one", async () => {
    render(<ServerSettingsCards />);
    const input = screen.getByLabelText(
      "Reading lists per user from a URL or catalog",
    ) as HTMLInputElement;
    expect(input.value).toBe("500");

    const card = input.closest("[data-slot=card]");
    expect(card).not.toBeNull();
    const save = Array.from(card!.querySelectorAll("button")).find(
      (b) => b.textContent === "Save",
    ) as HTMLButtonElement;
    expect(save.disabled).toBe(true);

    fireEvent.change(input, { target: { value: "0" } });
    expect(screen.getByText("Must be in [1, 10000].")).toBeTruthy();
    expect(save.disabled).toBe(true);

    fireEvent.change(input, { target: { value: "800" } });
    expect(save.disabled).toBe(false);
    fireEvent.click(save);
    await waitFor(() =>
      expect(mutateAsync).toHaveBeenCalledWith({
        "cbl.max_remote_lists_per_user": 800,
      }),
    );
  });
});
