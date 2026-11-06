// @vitest-environment jsdom
/**
 * "Scan all" lands on the live view of the batch it started
 * (scan-activity consolidation, 2026-11). The owner reported that clicking
 * Scan all left them where they were; the button must push to
 * `/admin/scans?batch=<id>` — the page that shows every member library's
 * live progress and, once the batch closes, the results.
 *
 * Real `useScanAllLibraries` + `useApiMutation` + QueryClient; only the
 * network (`apiFetch`) and the router are faked, so the whole success path
 * (hook-level toast + invalidations, then the call-site navigation) runs.
 */
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const nav = vi.hoisted(() => ({ push: vi.fn() }));
vi.mock("next/navigation", () => ({
  useRouter: () => ({ push: nav.push, replace: vi.fn(), refresh: vi.fn() }),
  usePathname: () => "/admin/libraries",
  useSearchParams: () => new URLSearchParams(),
}));

const net = vi.hoisted(() => ({
  posts: [] as { path: string; body: unknown }[],
}));
vi.mock("@/lib/api/auth-refresh", () => ({
  apiFetch: vi.fn(async (path: string, init?: RequestInit) => {
    const json = (body: unknown) =>
      new Response(JSON.stringify(body), {
        status: 200,
        headers: { "Content-Type": "application/json" },
      });
    if (init?.method === "POST") {
      net.posts.push({
        path,
        body: init.body ? JSON.parse(String(init.body)) : null,
      });
      return json({
        batch_id: "0193-batch",
        enqueued: [],
        newly_enqueued: 2,
        already_running: 0,
        failed: 0,
        total: 2,
        force: false,
      });
    }
    // GET /libraries — two libraries, so the button is enabled.
    return json([
      { id: "a", slug: "a", name: "A" },
      { id: "b", slug: "b", name: "B" },
    ]);
  }),
  getCsrfToken: () => "csrf",
}));

import { ScanAllButton } from "@/components/admin/library/ScanAllButton";

function renderButton() {
  const qc = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={qc}>
      <ScanAllButton />
    </QueryClientProvider>,
  );
}

describe("<ScanAllButton>", () => {
  beforeEach(() => {
    nav.push.mockReset();
    net.posts.length = 0;
  });

  it("starts the batch and navigates to its live Scans view", async () => {
    renderButton();
    const trigger = await screen.findByRole("button", {
      name: "Scan all libraries",
    });
    await waitFor(() =>
      expect((trigger as HTMLButtonElement).disabled).toBe(false),
    );
    fireEvent.click(trigger);
    fireEvent.click(await screen.findByRole("button", { name: "Scan all" }));

    await waitFor(() =>
      expect(nav.push).toHaveBeenCalledWith("/admin/scans?batch=0193-batch"),
    );
    expect(net.posts).toEqual([
      { path: "/admin/libraries/scan-all", body: { force: false } },
    ]);
    // The confirm dialog closes on the way out.
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
  });

  it("carries the content-verify choice into the request", async () => {
    renderButton();
    const trigger = await screen.findByRole("button", {
      name: "Scan all libraries",
    });
    await waitFor(() =>
      expect((trigger as HTMLButtonElement).disabled).toBe(false),
    );
    fireEvent.click(trigger);
    fireEvent.click(screen.getByLabelText("Content-verify scan"));
    fireEvent.click(
      screen.getByRole("button", { name: "Scan all (content-verify)" }),
    );
    await waitFor(() => expect(nav.push).toHaveBeenCalledTimes(1));
    expect(net.posts[0]?.body).toEqual({ force: true });
  });
});
