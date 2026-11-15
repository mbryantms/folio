// @vitest-environment jsdom
/**
 * Every cover card draws its cover in the shared `CoverFrame` (WP-9.2):
 * with the account's "Cover tilt and sheen" effect on it is a tilting
 * box with a sheen, and with the effect off, or under reduced motion, a
 * plain box. Either way it holds a `.cover-box`, the element the hover
 * glow lands on.
 *
 * The cards under test are the ones `docs/dev/card-corner-conventions.md`
 * lists; the last test fails when the doc names a card this file does
 * not render, so a new cover card cannot skip the frame unnoticed.
 */
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type * as QueriesMe from "@/lib/api/queries-me";

const account = vi.hoisted(() => ({
  me: {
    data: {
      effects: { cover_tilt: true } as Record<string, boolean>,
      motion: null as string | null,
    },
  },
}));

// The real `useVisualEffect` runs, so the effect switch and both kinds of
// reduced motion are the ones the app uses.
vi.mock("@/lib/api/queries-me", async (importOriginal) => ({
  ...(await importOriginal<typeof QueriesMe>()),
  useMe: () => account.me,
}));
vi.mock("@/lib/api/auth-refresh", () => ({
  getCsrfToken: () => null,
  apiFetch: async () =>
    new Response(JSON.stringify({ items: [], records: [] }), {
      status: 200,
      headers: { "Content-Type": "application/json" },
    }),
}));
vi.mock("next/navigation", () => ({
  useRouter: () => ({ push: () => {} }),
}));
vi.mock("@/lib/api/mutations", () => ({
  useDismissRailItem: () => ({ mutate: () => {}, isPending: false }),
  useUpsertIssueProgress: () => ({ mutate: () => {}, isPending: false }),
  useUpsertSeriesProgress: () => ({ mutate: () => {}, isPending: false }),
}));
vi.mock("@/components/CoverLongPressActions", () => ({
  useCoverLongPressActions: () => ({ wrapperProps: {}, sheet: null }),
}));
vi.mock("@/components/collections/useCoverMenuCollectionActions", () => ({
  useCoverMenuCollectionActions: () => ({ actions: [], dialog: null }),
}));
vi.mock("@/components/CoverMenuButton", () => ({
  CoverMenuButton: () => null,
}));
vi.mock("@/lib/ui/use-share-link", () => ({
  useShareLink: () => ({ label: "Share", shareOrCopy: () => {} }),
}));

import { CblIssueCard } from "@/components/cbl/cbl-issue-card";
import { CblWindowCard } from "@/components/cbl/CblWindowCard";
import { CoverFrame } from "@/components/library/CoverFrame";
import { IssueCard } from "@/components/library/IssueCard";
import { OnDeckCard } from "@/components/library/OnDeckCard";
import { ProgressIssueCard } from "@/components/library/ProgressIssueCard";
import { SeriesCard } from "@/components/library/SeriesCard";
import { UpcomingCard } from "@/components/upcoming/UpcomingCard";

const T0 = "2026-11-01T10:00:00Z";

const issue = {
  id: "i1",
  slug: "issue-1",
  series_id: "s1",
  series_slug: "saga",
  series_name: "Saga",
  title: "Chapter One",
  number: "1",
  sort_number: 1,
  year: 2012,
  page_count: 22,
  state: "active",
  cover_url: "/issues/i1/pages/0/thumb",
  created_at: T0,
  updated_at: T0,
};

const series = {
  id: "s1",
  slug: "saga",
  name: "Saga",
  status: "Active",
  publisher: "Image",
  issue_count: 3,
  cover_url: "/issues/i1/pages/0/thumb",
  created_at: T0,
  updated_at: T0,
};

/** One entry per card the conventions doc lists. */
const CARDS: Record<string, () => React.ReactElement> = {
  IssueCard: () => <IssueCard issue={issue as never} />,
  SeriesCard: () => <SeriesCard series={series as never} />,
  CblIssueCard: () => (
    <CblIssueCard
      entry={
        {
          id: "e1",
          position: 0,
          match_status: "matched",
          matched_issue_id: "i1",
          series_name: "Saga",
          issue_number: "1",
        } as never
      }
      issue={issue as never}
      cblSavedViewId="v1"
    />
  ),
  CblWindowCard: () => (
    <CblWindowCard
      entry={
        {
          position: 0,
          issue,
          finished: false,
          last_page: 0,
          percent: 0,
        } as never
      }
      isCurrent
      cblSavedViewId="v1"
    />
  ),
  ProgressIssueCard: () => (
    <ProgressIssueCard
      card={
        {
          issue,
          series_name: "Saga",
          progress: { last_page: 4, percent: 0.2, finished: false },
        } as never
      }
    />
  ),
  OnDeckCard: () => (
    <OnDeckCard
      card={{ kind: "series_next", issue, series_name: "Saga" } as never}
    />
  ),
  UpcomingCard: () => (
    <UpcomingCard
      entry={
        {
          key: "k1",
          number: "7",
          series_name: "Saga",
          publisher: "Image",
          release_date: "2026-11-18",
          stage: "announced",
          cover_url: "https://static.example.test/cover.jpg",
        } as never
      }
      onOpen={() => {}}
    />
  ),
};

function setOsReduced(matches: boolean) {
  window.matchMedia = vi.fn().mockImplementation((query: string) => ({
    matches: query.includes("reduced-motion") ? matches : false,
    media: query,
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
    addListener: vi.fn(),
    removeListener: vi.fn(),
  })) as unknown as typeof window.matchMedia;
}

function renderCard(name: string) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>{CARDS[name]!()}</QueryClientProvider>,
  ).container;
}

beforeEach(() => {
  setOsReduced(false);
  account.me = { data: { effects: { cover_tilt: true }, motion: null } };
});
afterEach(cleanup);

describe.each(Object.keys(CARDS))("%s", (name) => {
  it("tilts and shines with the effect on", () => {
    const container = renderCard(name);
    const frames = container.querySelectorAll("[data-cover-frame]");
    expect(frames).toHaveLength(1);
    const frame = frames[0]!;
    expect(frame.getAttribute("data-cover-frame")).toBe("card");
    expect(frame.classList.contains("cover-tilt")).toBe(true);
    expect(frame.classList.contains("relative")).toBe(true);
    expect(frame.querySelector(":scope > .cover-tilt-sheen")).toBeTruthy();
    // The cover box (placeholder, glow target) is inside the frame.
    expect(frame.querySelector(".cover-box")).toBeTruthy();
    // The card is the element that takes the cover's colour.
    expect(frame.closest("[data-card]")).toBeTruthy();
  });

  it("is a plain box with the effect off", () => {
    account.me = { data: { effects: { cover_tilt: false }, motion: null } };
    const container = renderCard(name);
    const frame = container.querySelector("[data-cover-frame]")!;
    expect(frame).toBeTruthy();
    expect(container.querySelector(".cover-tilt")).toBeNull();
    expect(container.querySelector(".cover-tilt-sheen")).toBeNull();
    expect(frame.querySelector(".cover-box")).toBeTruthy();
  });

  it("is a plain box under the account's reduced motion", () => {
    account.me = { data: { effects: { cover_tilt: true }, motion: "reduced" } };
    const container = renderCard(name);
    expect(container.querySelector("[data-cover-frame]")).toBeTruthy();
    expect(container.querySelector(".cover-tilt")).toBeNull();
  });

  it("is a plain box when the OS asks for reduced motion", () => {
    setOsReduced(true);
    account.me = { data: { effects: { cover_tilt: true }, motion: "system" } };
    const container = renderCard(name);
    expect(container.querySelector("[data-cover-frame]")).toBeTruthy();
    expect(container.querySelector(".cover-tilt")).toBeNull();
  });
});

describe("CoverFrame", () => {
  it("leans a hero cover half as far as a card", () => {
    const { container } = render(
      <>
        <CoverFrame data-testid="card">
          <div className="cover-box" />
        </CoverFrame>
        <CoverFrame lean="hero" data-testid="hero">
          <div className="cover-box" />
        </CoverFrame>
      </>,
    );
    const lean = (id: string) => {
      const el = container.querySelector<HTMLElement>(`[data-testid="${id}"]`)!;
      el.getBoundingClientRect = () =>
        ({ left: 0, top: 0, width: 100, height: 150 }) as DOMRect;
      // Pointer at the right edge, vertical centre: the largest Y lean.
      const move = new MouseEvent("pointermove", {
        bubbles: true,
        clientX: 100,
        clientY: 75,
      });
      Object.defineProperty(move, "pointerType", { value: "mouse" });
      const raf = vi
        .spyOn(window, "requestAnimationFrame")
        .mockImplementation((cb) => {
          cb(0);
          return 0;
        });
      el.dispatchEvent(move);
      raf.mockRestore();
      return el.style.getPropertyValue("--tilt-y");
    };
    expect(lean("card")).toBe("7.00deg");
    expect(lean("hero")).toBe("3.50deg");
  });

  it("warns in development when nothing inside draws a cover box", () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    render(
      <CoverFrame>
        <div />
      </CoverFrame>,
    );
    expect(warn).toHaveBeenCalledWith(expect.stringContaining(".cover-box"));
    warn.mockRestore();
  });
});

describe("the conventions doc", () => {
  it("lists no cover card this file does not render", () => {
    const doc = readFileSync(
      resolve(__dirname, "../../../docs/dev/card-corner-conventions.md"),
      "utf8",
    );
    // The opening sentence: "The cover cards across Folio (`A`, `B`, …)".
    const list = /The cover cards across Folio \(([^)]*)\)/.exec(doc);
    expect(list, "the doc's list of cover cards").toBeTruthy();
    const named = [...list![1]!.matchAll(/`(\w+)`/g)].map((m) => m[1]!);
    expect(named.length).toBeGreaterThan(0);
    expect(named.sort()).toEqual(Object.keys(CARDS).sort());
  });
});
