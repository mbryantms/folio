// @vitest-environment jsdom
/**
 * The findings page filtered to one scan run (`?scan_id=` from a scan
 * row's link): the filter goes to the server as a query param, the page
 * says which run it shows, and Clear drops only that param.
 */
import { fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

const SCAN = "0190a4f2-0000-7000-8000-00000000abcd";

let search = new URLSearchParams();
const replace = vi.fn();
vi.mock("next/navigation", () => ({
  useRouter: () => ({ replace }),
  useSearchParams: () => search,
  usePathname: () => "/admin/findings",
}));

const useAdminHealthIssues = vi.fn();
vi.mock("@/lib/api/queries", () => ({
  useAdminHealthIssues: (filters: unknown) => useAdminHealthIssues(filters),
  useLibraryList: () => ({ data: [] }),
  useLibraryEventsInfinite: () => ({ data: undefined, isLoading: false }),
}));
vi.mock("@/components/folio/infinite-sentinel", () => ({
  InfiniteSentinel: () => null,
}));
vi.mock("@/lib/api/mutations", () => ({
  useDismissHealthIssue: () => ({ mutate: vi.fn(), isPending: false }),
}));

function page(total: number | null, items: unknown[] = []) {
  return {
    data: { pages: [{ items, next_cursor: null, total }] },
    isLoading: false,
    error: null,
    hasNextPage: false,
    isFetchingNextPage: false,
    fetchNextPage: vi.fn(),
  };
}

afterEach(() => {
  useAdminHealthIssues.mockReset();
  replace.mockReset();
});

describe("FindingsView scan filter", () => {
  it("sends scan_id to the server and names the run", async () => {
    search = new URLSearchParams(`tab=health&scan_id=${SCAN}`);
    useAdminHealthIssues.mockReturnValue(page(0));
    const { FindingsView } =
      await import("@/components/admin/findings/FindingsView");
    render(<FindingsView />);

    expect(useAdminHealthIssues).toHaveBeenCalledWith(
      expect.objectContaining({ scan_id: SCAN }),
    );
    const notice = screen.getByTestId("findings-scan-filter");
    expect(notice.textContent).toContain("Showing findings from scan run");
    expect(notice.textContent).toContain(SCAN.slice(0, 8));
    expect(
      screen.getByText("This scan run has no open findings."),
    ).toBeTruthy();
  });

  it("clears only the scan filter", async () => {
    search = new URLSearchParams(`tab=health&severity=error&scan_id=${SCAN}`);
    useAdminHealthIssues.mockReturnValue(page(0));
    const { FindingsView } =
      await import("@/components/admin/findings/FindingsView");
    render(<FindingsView />);
    fireEvent.click(screen.getByRole("button", { name: /clear scan filter/i }));
    expect(replace).toHaveBeenCalledOnce();
    const url = replace.mock.calls[0]![0] as string;
    expect(url).toContain("severity=error");
    expect(url).not.toContain("scan_id");
  });

  it("shows no notice and sends no scan_id without the param", async () => {
    search = new URLSearchParams("tab=health");
    useAdminHealthIssues.mockReturnValue(page(0));
    const { FindingsView } =
      await import("@/components/admin/findings/FindingsView");
    render(<FindingsView />);
    expect(screen.queryByTestId("findings-scan-filter")).toBeNull();
    expect(useAdminHealthIssues).toHaveBeenCalledWith(
      expect.objectContaining({ scan_id: undefined }),
    );
  });

  it("shows the server's first-page total", async () => {
    search = new URLSearchParams(`tab=health&scan_id=${SCAN}`);
    useAdminHealthIssues.mockReturnValue(
      page(3, [
        {
          id: "r1",
          kind: "ArchiveSafetyRefusal",
          severity: "error",
          fingerprint: "f",
          payload: { kind: "archive_safety_refusal", data: { path: "/x.cbz" } },
          first_seen_at: "2026-11-10T00:00:00Z",
          last_seen_at: "2026-11-10T00:00:00Z",
          resolved_at: null,
          dismissed_at: null,
          scan_id: SCAN,
          library_id: "lib",
          library_name: "Comics",
          library_slug: "comics",
        },
      ]),
    );
    const { FindingsView } =
      await import("@/components/admin/findings/FindingsView");
    render(<FindingsView />);
    expect(screen.getByTestId("findings-total").textContent).toBe("3 findings");
    expect(screen.getByText("Archive refused by a safety limit")).toBeTruthy();
  });
});
