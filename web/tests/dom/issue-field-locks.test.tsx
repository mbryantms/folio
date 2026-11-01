// @vitest-environment jsdom
/**
 * `<IssueFieldLocks>` (audit 2026-11 §5 #5): one lock toggle per lockable
 * field for admins; an unlocked toggle PUTs the lock, a locked one DELETEs
 * it, each with its own toast and an overview refetch. Readers only see
 * which fields are locked.
 */
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const { apiFetch, refresh, toastSuccess } = vi.hoisted(() => ({
  apiFetch: vi.fn(),
  refresh: vi.fn(),
  toastSuccess: vi.fn(),
}));
vi.mock("@/lib/api/auth-refresh", () => ({ apiFetch }));
vi.mock("next/navigation", () => ({
  useRouter: () => ({ refresh, push: vi.fn(), replace: vi.fn() }),
  usePathname: () => "/series/saga/issues/saga-1",
}));
vi.mock("sonner", () => ({
  toast: Object.assign(vi.fn(), {
    success: toastSuccess,
    error: vi.fn(),
    message: vi.fn(),
    info: vi.fn(),
    loading: vi.fn(),
    dismiss: vi.fn(),
  }),
}));

import { IssueFieldLocks } from "@/app/[locale]/(library)/series/[slug]/issues/[issueSlug]/IssueFieldLocks";
import { queryKeys } from "@/lib/api/queries";

function renderLocks(canEdit: boolean) {
  const qc = new QueryClient({
    defaultOptions: { mutations: { retry: false } },
  });
  const invalidate = vi.spyOn(qc, "invalidateQueries");
  render(
    <QueryClientProvider client={qc}>
      <IssueFieldLocks
        seriesSlug="saga"
        issueSlug="saga-1"
        lockableFields={["title", "summary", "credits"]}
        lockedFields={["credits"]}
        otherLockedFields={["black_and_white"]}
        canEdit={canEdit}
      />
    </QueryClientProvider>,
  );
  return { invalidate };
}

describe("<IssueFieldLocks>", () => {
  beforeEach(() => {
    apiFetch.mockReset();
    refresh.mockReset();
    toastSuccess.mockReset();
    apiFetch.mockResolvedValue(
      new Response(JSON.stringify({ field: "title", pinned_keys: ["title"] }), {
        status: 200,
        headers: { "Content-Type": "application/json" },
      }),
    );
  });

  it("renders one lock toggle per lockable field with its state", () => {
    renderLocks(true);
    const title = screen.getByRole("button", {
      name: "Lock Title against provider updates",
    });
    const credits = screen.getByRole("button", {
      name: "Lock Credits against provider updates",
    });
    expect(title.getAttribute("aria-pressed")).toBe("false");
    expect(credits.getAttribute("aria-pressed")).toBe("true");
    // A pin outside the lockable set is listed as locked so it can be
    // released.
    expect(
      screen
        .getByRole("button", {
          name: "Lock Black & white against provider updates",
        })
        .getAttribute("aria-pressed"),
    ).toBe("true");
    expect(
      screen.getAllByRole("button", { name: /against provider updates$/ }),
    ).toHaveLength(4);
  });

  it("locks an unlocked field with PUT", async () => {
    const { invalidate } = renderLocks(true);
    fireEvent.click(
      screen.getByRole("button", {
        name: "Lock Title against provider updates",
      }),
    );
    await waitFor(() => expect(toastSuccess).toHaveBeenCalled());
    expect(apiFetch).toHaveBeenCalledWith(
      "/series/saga/issues/saga-1/field-provenance/title",
      expect.objectContaining({ method: "PUT" }),
    );
    expect(toastSuccess.mock.calls[0]?.[0]).toBe("Field locked");
    expect(invalidate).toHaveBeenCalledWith({
      queryKey: queryKeys.issueMetadataOverview("saga", "saga-1"),
    });
    expect(invalidate).toHaveBeenCalledWith({
      queryKey: queryKeys.issueMetadataMatchAll("saga", "saga-1"),
    });
    expect(refresh).toHaveBeenCalled();
  });

  it("unlocks a locked field with DELETE", async () => {
    apiFetch.mockResolvedValue(
      new Response(JSON.stringify({ cleared: true }), {
        status: 200,
        headers: { "Content-Type": "application/json" },
      }),
    );
    renderLocks(true);
    fireEvent.click(
      screen.getByRole("button", {
        name: "Lock Credits against provider updates",
      }),
    );
    await waitFor(() => expect(toastSuccess).toHaveBeenCalled());
    expect(apiFetch).toHaveBeenCalledWith(
      "/series/saga/issues/saga-1/field-provenance/credits",
      expect.objectContaining({ method: "DELETE" }),
    );
    expect(toastSuccess.mock.calls[0]?.[0]).toBe("Field unlocked");
  });

  it("shows readers the locked fields only, with no controls", () => {
    renderLocks(false);
    expect(screen.queryAllByRole("button")).toHaveLength(0);
    expect(screen.getByText("Credits")).toBeTruthy();
    expect(screen.getByText("Black & white")).toBeTruthy();
    expect(screen.queryByText("Title")).toBeNull();
    expect(
      screen.getAllByRole("img", {
        name: "Locked: provider updates won't change this field",
      }),
    ).toHaveLength(2);
  });
});
