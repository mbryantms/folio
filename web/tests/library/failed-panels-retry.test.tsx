// @vitest-environment jsdom
/**
 * Panels whose fetch failed say so in a fixed sentence and offer Retry
 * (the `RailError` pattern) instead of printing the error object or
 * telling the reader to refresh the page. Real hooks + real QueryClient;
 * only the network (`apiFetch`) is faked, and it always answers 500 so
 * every Retry click shows up as one more request.
 */
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { ReactElement } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const net = vi.hoisted(() => ({ urls: [] as string[] }));

vi.mock("@/lib/api/auth-refresh", () => ({
  apiFetch: vi.fn(async (path: string) => {
    net.urls.push(path);
    return new Response(
      JSON.stringify({ error: { code: "internal", message: "boom" } }),
      { status: 500, headers: { "Content-Type": "application/json" } },
    );
  }),
  getCsrfToken: () => "csrf",
}));

vi.mock("next/navigation", () => ({
  useRouter: () => ({ push: vi.fn(), replace: vi.fn(), refresh: vi.fn() }),
  usePathname: () => "/",
  useSearchParams: () => new URLSearchParams(),
}));

import { CblDetail } from "@/components/cbl/cbl-detail";
import { AppearancesTab } from "@/components/library/AppearancesTab";
import { EntityInsights } from "@/components/library/EntityInsights";
import type { SavedViewView } from "@/lib/api/types";

function renderWithClient(ui: ReactElement) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>{ui}</QueryClientProvider>,
  );
}

async function expectRetryRefetches(message: string, urlPart: string) {
  expect(await screen.findByText(message)).toBeTruthy();
  // Neither the error class, the status, nor the server message leaks.
  expect(document.body.textContent).not.toMatch(/HttpError|Error:|500|boom/);
  const hits = () => net.urls.filter((u) => u.includes(urlPart)).length;
  const before = hits();
  fireEvent.click(screen.getByRole("button", { name: "Retry" }));
  await waitFor(() => expect(hits()).toBe(before + 1));
}

beforeEach(() => {
  net.urls.length = 0;
});

describe("failed panels offer Retry", () => {
  it("CBL detail: no raw error, Retry refetches the list", async () => {
    renderWithClient(
      <CblDetail
        savedView={{ id: "v-1", cbl_list_id: "l-1" } as SavedViewView}
      />,
    );
    await expectRetryRefetches(
      "Couldn't load this reading list.",
      "/me/cbl-lists/l-1",
    );
  });

  it("entity insights: Retry instead of 'Try refreshing'", async () => {
    renderWithClient(<EntityInsights kind="characters" slug="batman" />);
    await expectRetryRefetches(
      "Couldn't load library statistics.",
      "/characters/batman/insights",
    );
    expect(screen.queryByText(/Try refreshing/)).toBeNull();
  });

  it("appearances tab: Retry refetches", async () => {
    renderWithClient(<AppearancesTab variant="series" seriesSlug="saga" />);
    await expectRetryRefetches(
      "Couldn't load where this series appears.",
      "saga",
    );
  });
});
