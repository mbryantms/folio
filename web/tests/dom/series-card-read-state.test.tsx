// @vitest-environment jsdom
/**
 * Series cards show the viewer's read state (U-03): a started series gets
 * the issue cards' bottom progress bar, a finished one the top-right read
 * check, an untouched one neither — each with words a screen reader gets.
 * A payload without `progress_summary` shows no state. The collection
 * badge carries its meaning in an icon + label and still honours the
 * "Collection badge" preference. Every state keeps the card's box: the
 * additions are absolutely positioned and the caption is unchanged.
 */
import { render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup } from "@testing-library/react";

const dotState = vi.hoisted(() => ({ enabled: true }));

function json(body: unknown) {
  return new Response(JSON.stringify(body), {
    status: 200,
    headers: { "content-type": "application/json" },
  });
}

vi.mock("@/lib/api/auth-refresh", () => ({
  getCsrfToken: () => "csrf",
  apiFetch: async (path: string) => {
    if (path.startsWith("/auth/me")) return json({ id: "u1", role: "user" });
    if (path.startsWith("/me/collections")) return json([]);
    return json({ items: [], next_cursor: null });
  },
}));
vi.mock("next/navigation", () => ({
  useRouter: () => ({ push: vi.fn(), replace: vi.fn(), refresh: vi.fn() }),
  usePathname: () => "/library",
  useSearchParams: () => new URLSearchParams(),
}));
vi.mock("next/link", () => ({
  default: ({
    href,
    children,
    ...rest
  }: React.AnchorHTMLAttributes<HTMLAnchorElement> & { href: string }) => (
    <a href={href} {...rest}>
      {children}
    </a>
  ),
}));
vi.mock("@/lib/visual-effects", () => ({
  useVisualEffect: () => false,
  useReducedMotion: () => true,
}));
vi.mock("@/components/collections/useCoverMenuCollectionActions", () => ({
  useCoverMenuCollectionActions: () => ({ actions: [], dialog: null }),
}));
vi.mock("@/components/library/use-cover-collection-dot", () => ({
  useCoverCollectionDot: () => ({
    enabled: dotState.enabled,
    setEnabled: () => {},
  }),
}));

import {
  SeriesCard,
  SeriesCardSkeleton,
} from "@/components/library/SeriesCard";
import type { SeriesView } from "@/lib/api/types";
import { seriesReadState } from "@/lib/series-read-state";

function seriesWith(
  summary: { finished: number; in_progress: number; total: number } | null,
  extra: Partial<SeriesView> = {},
): SeriesView {
  return {
    id: "s-1",
    slug: "saga",
    name: "Saga",
    year: 2012,
    issue_count: 5,
    cover_url: null,
    status: "continuing",
    progress_summary: summary
      ? { ...summary, finished_pages: 0 }
      : (summary as null),
    ...extra,
  } as unknown as SeriesView;
}

function renderCard(series: SeriesView) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <SeriesCard series={series} />
    </QueryClientProvider>,
  );
}

/** The caption block under the cover (title + meta line). */
function caption(container: HTMLElement): string {
  const link = container.querySelector("[data-card]")!;
  return (link.lastElementChild as HTMLElement).outerHTML;
}

afterEach(() => {
  cleanup();
  dotState.enabled = true;
});

describe("seriesReadState", () => {
  it("classifies untouched, started and finished", () => {
    expect(
      seriesReadState({ finished: 0, in_progress: 0, total: 5 } as never),
    ).toEqual({ kind: "unread", label: "Unread" });
    expect(
      seriesReadState({ finished: 3, in_progress: 1, total: 5 } as never),
    ).toEqual({
      kind: "started",
      label: "3 of 5 issues read, 1 in progress",
      percent: 60,
    });
    expect(
      seriesReadState({ finished: 5, in_progress: 0, total: 5 } as never),
    ).toEqual({ kind: "finished", label: "All 5 issues read" });
  });

  it("gives an in-progress-only series a visible sliver", () => {
    const s = seriesReadState({
      finished: 0,
      in_progress: 1,
      total: 40,
    } as never);
    expect(s?.kind).toBe("started");
    expect(s && "percent" in s ? s.percent : 0).toBeGreaterThan(0);
  });

  it("says nothing without a summary or without active issues", () => {
    expect(seriesReadState(null)).toBeNull();
    expect(seriesReadState(undefined)).toBeNull();
    expect(
      seriesReadState({ finished: 0, in_progress: 0, total: 0 } as never),
    ).toBeNull();
  });
});

describe("SeriesCard read state", () => {
  it("started: a bottom progress bar with an accessible label", () => {
    const { container } = renderCard(
      seriesWith({ finished: 3, in_progress: 0, total: 5 }),
    );
    const bar = screen.getByRole("img", { name: "3 of 5 issues read" });
    expect(bar.getAttribute("data-read-state")).toBe("started");
    expect(bar.className).toContain("absolute");
    expect(bar.className).toContain("bottom-0");
    expect((bar.firstElementChild as HTMLElement).style.width).toBe("60%");
    expect(screen.queryByRole("img", { name: /All .* read/ })).toBeNull();
    expect(container.querySelector('[data-read-state="unread"]')).toBeNull();
    // The link's accessible name carries the state.
    expect(
      screen.getByRole("link", { name: /3 of 5 issues read/ }),
    ).toBeTruthy();
  });

  it("finished: the read check in the top-right stack", () => {
    const { container } = renderCard(
      seriesWith({ finished: 5, in_progress: 0, total: 5 }),
    );
    const check = screen.getByRole("img", { name: "All 5 issues read" });
    expect(check.parentElement?.className).toContain("top-2 right-2");
    expect(container.querySelector('[data-read-state="started"]')).toBeNull();
    expect(
      screen.getByRole("link", { name: /All 5 issues read/ }),
    ).toBeTruthy();
  });

  it("untouched: no mark on the cover, visually hidden 'Unread'", () => {
    const { container } = renderCard(
      seriesWith({ finished: 0, in_progress: 0, total: 5 }),
    );
    const hidden = container.querySelector('[data-read-state="unread"]');
    expect(hidden?.className).toContain("sr-only");
    expect(hidden?.textContent).toBe("Unread");
    expect(container.querySelector('[data-read-state="started"]')).toBeNull();
    expect(screen.queryByRole("img", { name: /read$/ })).toBeNull();
    expect(screen.getByRole("link", { name: /Unread/ })).toBeTruthy();
  });

  it("no progress_summary: no read state at all", () => {
    const { container } = renderCard(seriesWith(null));
    expect(container.querySelector("[data-read-state]")).toBeNull();
    expect(screen.queryByRole("img", { name: /read/i })).toBeNull();
  });

  it("every state keeps the same caption (no layout shift)", () => {
    const captions = [
      seriesWith({ finished: 0, in_progress: 0, total: 5 }),
      seriesWith({ finished: 3, in_progress: 0, total: 5 }),
      seriesWith({ finished: 5, in_progress: 0, total: 5 }),
      seriesWith(null),
    ].map((s) => {
      const { container } = renderCard(s);
      const html = caption(container);
      cleanup();
      return html;
    });
    expect(new Set(captions).size).toBe(1);
  });
});

describe("SeriesCard collection badge", () => {
  const owned = { total_issues: 5, main_issue_count: 5, issue_count: 5 };
  const missing = { total_issues: 8, main_issue_count: 5, issue_count: 5 };

  it("complete and incomplete differ by icon and accessible name", () => {
    const a = renderCard(seriesWith(null, owned));
    const complete = screen.getByRole("img", {
      name: "Collection complete: 5 of 5 issues",
    });
    expect(complete.getAttribute("data-collection")).toBe("complete");
    const completeIcon = complete.querySelector("svg")!.getAttribute("class");
    a.unmount();

    renderCard(seriesWith(null, missing));
    const incomplete = screen.getByRole("img", {
      name: "Collection incomplete: 5 of 8 issues",
    });
    const incompleteIcon = incomplete
      .querySelector("svg")!
      .getAttribute("class");
    expect(completeIcon).not.toBe(incompleteIcon);
  });

  it("the preference still hides it", () => {
    dotState.enabled = false;
    renderCard(seriesWith(null, owned));
    expect(screen.queryByRole("img", { name: /Collection/ })).toBeNull();
  });
});

describe("SeriesCardSkeleton", () => {
  it("keeps the loaded card's box: same wrapper, cover and two lines", () => {
    const { container } = render(<SeriesCardSkeleton />);
    const root = container.firstElementChild as HTMLElement;
    expect(root.className).toContain("flex shrink-0 flex-col gap-2 p-1");
    expect(root.className).toContain("w-full");
    expect(root.children).toHaveLength(2);
    expect(root.children[1]!.children).toHaveLength(2);
  });
});
