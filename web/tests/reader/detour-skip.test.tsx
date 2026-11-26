// @vitest-environment jsdom
/**
 * "Skip" on the end card's event rows: an event linked into the list
 * being read can be skipped from the card (the reader's own skip, the
 * same `PUT …/links/{id}/skip` the list page uses) and disappears at
 * once. A collection's list entry has no parent list and gets no Skip.
 */
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { NextUpDetour } from "@/lib/api/types";

const apiMutate = vi.fn();
vi.mock("@/lib/api/mutations/_core", () => ({
  apiMutate: (...a: unknown[]) => apiMutate(...a),
}));
vi.mock("next/navigation", () => ({
  useRouter: () => ({ replace: vi.fn() }),
}));
vi.mock("sonner", () => ({
  toast: { message: vi.fn(), error: vi.fn() },
}));

import { DetourButtons } from "@/app/[locale]/read/[seriesSlug]/[issueSlug]/DetourButtons";

const detour = (over: Partial<NextUpDetour>): NextUpDetour => ({
  link_id: "link-1",
  list_id: "event-1",
  parent_list_id: "master-1",
  name: "Age of Ultron",
  view_id: "view-1",
  total: 10,
  in_library: 8,
  read: 0,
  first_unread: { issue_id: "i1", issue_slug: "1", series_slug: "aou" },
  ...over,
});

function show(detours: NextUpDetour[]) {
  return render(
    <QueryClientProvider client={new QueryClient()}>
      <DetourButtons detours={detours} viaViewId="view-m" />
    </QueryClientProvider>,
  );
}

describe("DetourButtons skip", () => {
  beforeEach(() => {
    apiMutate.mockReset();
    apiMutate.mockResolvedValue(null);
  });

  it("skips an event of the list and hides its row", async () => {
    show([
      detour({}),
      detour({ link_id: "link-2", list_id: "event-2", name: "Infinity" }),
    ]);
    fireEvent.click(screen.getByRole("button", { name: "Skip Age of Ultron" }));
    await waitFor(() =>
      expect(screen.queryByText(/Start Age of Ultron/)).toBeNull(),
    );
    expect(apiMutate).toHaveBeenCalledWith({
      path: "/me/cbl-lists/master-1/links/link-1/skip",
      method: "PUT",
    });
    expect(screen.getByText(/Start Infinity/)).toBeTruthy();
  });

  it("keeps the row when the request fails", async () => {
    apiMutate.mockRejectedValueOnce(new Error("offline"));
    show([detour({})]);
    fireEvent.click(screen.getByRole("button", { name: "Skip Age of Ultron" }));
    await waitFor(() => expect(apiMutate).toHaveBeenCalled());
    expect(screen.getByText(/Start Age of Ultron/)).toBeTruthy();
  });

  it("offers no Skip for a collection's list entry", () => {
    show([detour({ parent_list_id: undefined })]);
    expect(screen.queryByRole("button", { name: /^Skip/ })).toBeNull();
  });
});
