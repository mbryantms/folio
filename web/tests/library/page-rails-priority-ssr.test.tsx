/**
 * D18 review item 1: when the leading rail is a system rail the prefetch
 * found empty (Continue reading on a fresh account), the prefetch walks on
 * to the next rail — and the server HTML must then contain *that* rail's
 * cards with eager, high-priority covers, not a lazy placeholder. Server
 * render (`renderToString`, node env) of the real `PageRails` + rail
 * bodies under the real `PrefetchBoundary`; only page chrome is stubbed.
 */
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { renderToString } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";

vi.mock("@/lib/api/auth-refresh", () => ({
  apiFetch: vi.fn(async () => new Response("{}", { status: 500 })),
  getCsrfToken: () => "csrf",
}));
vi.mock("next/navigation", () => ({
  usePathname: () => "/",
  useRouter: () => ({ push: () => {}, replace: () => {}, prefetch: () => {} }),
  useSearchParams: () => new URLSearchParams(),
}));
// Page chrome around the rails: not under test.
vi.mock("@/components/library/CoverSizeOptions", () => ({
  CoverSizeOptions: () => null,
  CoverSizeFields: () => null,
}));
vi.mock("@/components/library/PhoneControlsBar", () => ({
  PhoneControlsBar: () => null,
}));
vi.mock("@/components/library/StickyToolbarRow", () => ({
  StickyToolbarRow: () => null,
}));
vi.mock("@/components/saved-views/PageActionsMenu", () => ({
  PageActionsMenu: () => null,
}));
vi.mock("@/components/saved-views/PageHeading", () => ({
  PageHeading: () => null,
}));

import { PrefetchBoundary } from "@/components/PrefetchBoundary";
import { PageRails } from "@/components/saved-views/PageRails";
import {
  createServerQueryClient,
  prefetchPageRails,
} from "@/lib/saved-views/prefetch-page-rails";

const series = (id: string) => ({
  id,
  slug: id,
  name: `Series ${id}`,
  cover_url: `/issues/${id}/pages/0/thumb`,
  library_id: "lib",
  issue_count: 3,
  status: "continuing",
});

const ROUTES: Record<string, unknown> = {
  "/me/saved-views?pinned_on=home": {
    items: [
      {
        id: "cr",
        name: "Continue reading",
        kind: "system",
        system_key: "continue_reading",
      },
      { id: "recent", name: "Recently added", kind: "filter_series" },
      { id: "later", name: "Later", kind: "filter_series" },
    ],
  },
  "/me/continue-reading": { items: [] },
  "/me/saved-views/recent/results": {
    items: [series("s1"), series("s2")],
    next_cursor: null,
  },
};

async function serverHtml() {
  const { state, snapshotId, priorityViewId } = await prefetchPageRails(
    createServerQueryClient(),
    {
      pageId: "home",
      isSystem: false,
      fetch: async <T,>(path: string): Promise<T> => {
        if (!(path in ROUTES)) throw new Error(`404 ${path}`);
        return ROUTES[path] as T;
      },
    },
  );
  // The browser-side provider's SSR pass: a fresh client per request.
  const client = new QueryClient({
    defaultOptions: { queries: { staleTime: 30_000, retry: false } },
  });
  return renderToString(
    <QueryClientProvider client={client}>
      <PrefetchBoundary state={state} snapshotId={snapshotId}>
        <PageRails
          pageId="home"
          pageName="Home"
          pageDescription={null}
          isSystem={false}
          showInSidebar={false}
          priorityViewId={priorityViewId}
        />
      </PrefetchBoundary>
    </QueryClientProvider>,
  );
}

describe("PageRails server HTML when the leading system rail is empty", () => {
  it("renders the next rail's cards with eager, high-priority covers", async () => {
    const html = await serverHtml();
    // The empty Continue reading rail renders nothing (no header, no
    // placeholder-height gap above the rail that shows).
    expect(html).not.toContain("Continue reading");
    expect(html).toContain("Recently added");
    expect(html).toContain("Series s1");
    expect(html).toContain("Series s2");
    const covers = html.match(
      /<img[^>]*\/issues\/s[12]\/pages\/0\/thumb[^>]*>/g,
    );
    expect(covers?.length).toBeGreaterThanOrEqual(2);
    for (const img of covers ?? []) {
      expect(img).toContain('loading="eager"');
      expect(img).toMatch(/fetchPriority="high"/i);
    }
    // Rails after it stay lazy placeholders.
    expect(html.match(/data-testid="lazy-rail-placeholder"/g)?.length).toBe(1);
  });
});
