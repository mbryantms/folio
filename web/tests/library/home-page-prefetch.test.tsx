/**
 * The home route's server prefetch is per request (D18). Render the page
 * for user A, then for user B, and assert B's dehydrated state carries
 * nothing of A's: this fails if `page.tsx` (or the helper) ever keeps a
 * module-level QueryClient. Also checks the page hands the settled rail to
 * `PageRails` and the snapshot id to the boundary.
 */
import type { DehydratedState } from "@tanstack/react-query";
import { describe, expect, it, vi } from "vitest";

const user = vi.hoisted(() => ({ current: "a" as "a" | "b" }));

const ROUTES: Record<"a" | "b", Record<string, unknown>> = {
  a: {
    "/me/pages": {
      items: [{ id: "page-a", name: "Home", is_system: true }],
      next_cursor: null,
    },
    "/me/saved-views?pinned_on=page-a": {
      items: [
        {
          id: "cr",
          name: "Continue reading",
          kind: "system",
          system_key: "continue_reading",
        },
      ],
    },
    "/me/continue-reading": { items: [{ issue: { id: "secret-a" } }] },
    "/libraries": [{ id: "lib-a" }],
  },
  b: {
    "/me/pages": {
      items: [{ id: "page-b", name: "Home", is_system: true }],
      next_cursor: null,
    },
    "/me/saved-views?pinned_on=page-b": {
      items: [{ id: "f-b", name: "Mine", kind: "filter_series" }],
    },
    "/me/saved-views/f-b/results": { items: [{ id: "series-b" }] },
    "/libraries": [{ id: "lib-b" }],
  },
};

vi.mock("@/lib/api/fetch", () => {
  class ApiError extends Error {
    constructor(
      public status: number,
      message: string,
    ) {
      super(message);
    }
  }
  return {
    ApiError,
    apiGet: vi.fn(async (path: string) => {
      const body = ROUTES[user.current][path];
      if (body === undefined) throw new ApiError(404, path);
      return body;
    }),
  };
});
vi.mock("next/navigation", () => ({
  redirect: (url: string) => {
    throw new Error(`redirect ${url}`);
  },
}));
// Client components: stubs that keep their props inspectable.
vi.mock("@/components/PrefetchBoundary", () => ({
  PrefetchBoundary: (p: unknown) => p,
}));
vi.mock("@/components/saved-views/PageRails", () => ({
  PageRails: (p: unknown) => p,
}));
vi.mock("@/components/saved-views/RailError", () => ({
  RailError: () => null,
}));
vi.mock("@/components/ScrollToTopOnMount", () => ({
  ScrollToTopOnMount: () => null,
}));
vi.mock("@/components/library/LibraryGridView", () => ({
  LibraryGridView: () => null,
}));

import HomePage from "@/app/[locale]/(library)/page";
import { PrefetchBoundary } from "@/components/PrefetchBoundary";
import { PageRails } from "@/components/saved-views/PageRails";

type El = { type: unknown; props: Record<string, unknown> };

function find(node: unknown, type: unknown): El | null {
  if (!node || typeof node !== "object") return null;
  const el = node as El;
  if (el.type === type) return el;
  const children = el.props?.children;
  for (const c of Array.isArray(children) ? children : [children]) {
    const hit = find(c, type);
    if (hit) return hit;
  }
  return null;
}

async function renderAs(who: "a" | "b") {
  user.current = who;
  const tree = await HomePage({ searchParams: Promise.resolve({}) });
  const boundary = find(tree, PrefetchBoundary);
  const rails = find(tree, PageRails);
  if (!boundary || !rails) throw new Error("home tree shape changed");
  return {
    state: boundary.props.state as DehydratedState,
    snapshotId: boundary.props.snapshotId as string,
    priorityViewId: rails.props.priorityViewId as string | null,
  };
}

describe("home page server prefetch", () => {
  it("never carries one user's data into another user's render", async () => {
    const a = await renderAs("a");
    const b = await renderAs("b");
    expect(JSON.stringify(a.state)).toContain("secret-a");
    const bWire = JSON.stringify(b.state);
    expect(bWire).not.toContain("secret-a");
    expect(bWire).not.toContain("lib-a");
    expect(bWire).not.toContain("page-a");
    expect(b.snapshotId).not.toBe(a.snapshotId);
  });

  it("passes the rail the prefetch settled on to PageRails", async () => {
    expect((await renderAs("a")).priorityViewId).toBe("cr");
    expect((await renderAs("b")).priorityViewId).toBe("f-b");
  });
});
