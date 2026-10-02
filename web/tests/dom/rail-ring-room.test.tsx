// @vitest-environment jsdom
/**
 * M7 UI polish: rings and focus indicators that paint *outside* a card's
 * box must not be clipped by the scrollers around it, and controlled
 * dialogs hand focus back to their opener.
 *
 *  - `HorizontalScrollRail`'s track keeps `RAIL_RING_ROOM_PX` (p-1) of
 *    room on every side, and the scroller's `-my-1` hands the vertical
 *    half back so no existing rail layout moves (`flow-root` stops the
 *    negative margin collapsing through the wrapper).
 *  - The Related tab's reading-order strip lives in that rail (an `<ol>`
 *    track), so the current series' `ring-2` highlight is never inside a
 *    bare `overflow-x-auto` box, and it carries `aria-current` +
 *    `data-rail-current` (centred on load).
 *  - Remove (AlertDialog) and Edit (Dialog), opened from per-card buttons
 *    with no Radix trigger, restore focus to that button on close.
 *
 * Class assertions are deliberate here: the bug was purely a missing
 * padding / margin pair, and jsdom has no layout to measure.
 */
import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { SeriesRelationshipsResp, SeriesView } from "@/lib/api/types";

let data: SeriesRelationshipsResp | undefined;

vi.mock("@/lib/api/queries", () => ({
  useRelationshipKinds: () => ({ data: undefined }),
  useMe: () => ({ data: { role: "admin" } }),
  useSeriesRelationships: () => ({ data, isLoading: false }),
  useSeriesListInfinite: () => ({ data: undefined, isLoading: false }),
  useEntityListInfinite: () => ({ data: undefined, isLoading: false }),
  useSeriesRelationshipSuggestions: () => ({
    data: undefined,
    isLoading: false,
  }),
}));
vi.mock("@/lib/api/mutations", () => ({
  useCreateSeriesRelationship: () => ({ mutate: vi.fn(), isPending: false }),
  useDeleteSeriesRelationship: () => ({ mutate: vi.fn(), isPending: false }),
  useUpdateSeriesRelationship: () => ({ mutate: vi.fn(), isPending: false }),
  useAcceptRelationshipSuggestion: () => ({
    mutate: vi.fn(),
    isPending: false,
  }),
  useRejectRelationshipSuggestion: () => ({
    mutate: vi.fn(),
    isPending: false,
  }),
}));

import {
  HorizontalScrollRail,
  RAIL_RING_ROOM_PX,
} from "@/components/library/HorizontalScrollRail";
import { SeriesRelatedSection } from "@/components/library/SeriesRelatedSection";

function series(id: string, name: string, year: number): SeriesView {
  return {
    id,
    slug: name.toLowerCase().replace(/\s+/g, "-"),
    name,
    year,
    library_id: "lib",
    status: "continuing",
    language_code: "en",
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
    cover_url: null,
  } as SeriesView;
}

const vol1 = series("s1", "Saga Vol 1", 2012);
const vol2 = series("s2", "Saga Vol 2", 2014);
const vol3 = series("s3", "Saga Vol 3", 2016);

beforeEach(() => {
  data = {
    series_id: "s2",
    relationships: [
      {
        id: "r3",
        kind: "continued_by",
        kind_label: "Continued by",
        group: "publication",
        source: "manual",
        confidence: null,
        created_at: "2026-01-01T00:00:00Z",
        series: vol3,
      },
    ],
    arcs: [],
    chain: [
      { position: -1, series: vol1 },
      { position: 0, series: vol2 },
      { position: 1, series: vol3 },
    ],
  } as SeriesRelationshipsResp;
});

describe("HorizontalScrollRail ring room", () => {
  it("pads the track on all sides and cancels the vertical half", () => {
    render(
      <HorizontalScrollRail viewAllHref="/all">
        <div>card</div>
      </HorizontalScrollRail>,
    );
    const scroller = screen.getByTestId("rail-scroller");
    const track = screen.getByTestId("rail-track");
    expect(RAIL_RING_ROOM_PX).toBe(4);
    // p-1 = 4px on every side (not just px-1): room for ring-2 + offset-2.
    expect(track.className.split(" ")).toContain("p-1");
    expect(track.className).not.toMatch(/\bpx-1\b/);
    // The scroller hands the vertical room back, so layouts don't move…
    expect(scroller.className.split(" ")).toContain("-my-1");
    expect(scroller.className).toContain("overflow-x-auto");
    // …and the wrapper is a BFC so the negative margin can't collapse
    // through it (the fades / chevrons stay anchored to the cards).
    expect(scroller.parentElement?.className.split(" ")).toContain("flow-root");
  });

  it("renders a labelled list track and wraps the View-all tile in an li", () => {
    render(
      <HorizontalScrollRail as="ol" trackLabel="Order" viewAllHref="/all">
        <li>card</li>
      </HorizontalScrollRail>,
    );
    const list = screen.getByRole("list", { name: "Order" });
    expect(list.tagName).toBe("OL");
    expect(list.className.split(" ")).toContain("p-1");
    for (const child of Array.from(list.children)) {
      expect(child.tagName).toBe("LI");
    }
  });

  it("keeps the chevrons on theme tokens (no white ring)", () => {
    render(
      <HorizontalScrollRail>
        <div>card</div>
      </HorizontalScrollRail>,
    );
    for (const name of ["Scroll left", "Scroll right"]) {
      const btn = screen.getByRole("button", { name });
      expect(btn.className).not.toContain("white");
      expect(btn.className).toContain("focus-visible:ring-ring");
    }
  });
});

describe("Reading order strip", () => {
  it("lives in the rail, with the current series marked and unclipped", () => {
    render(
      <SeriesRelatedSection
        seriesSlug="saga-vol-2"
        seriesId="s2"
        coverWidth={160}
      />,
    );
    const list = screen.getByRole("list", { name: "Reading order" });
    // The strip's own scroller is the rail's (ring room), not a bare
    // `overflow-x-auto` list.
    expect(list.className).not.toContain("overflow-x-auto");
    expect(list.className.split(" ")).toContain("p-1");
    expect(list.parentElement?.getAttribute("data-testid")).toBe(
      "rail-scroller",
    );
    const current = list.querySelector('[aria-current="true"]');
    expect(current?.getAttribute("data-rail-current")).toBe("true");
    expect(current?.textContent).toContain("This series");
    // The highlight is a ring on the cover; the card's focus ring uses
    // the ring token with an offset (outside the box → needs ring room).
    expect(current?.querySelector(".ring-primary")).toBeTruthy();
    const card = current?.querySelector('[data-testid="related-card"]');
    expect(card?.className).toContain("focus-visible:ring-ring");
    expect(card?.className).toContain("focus-visible:ring-offset-2");
  });
});

describe("Controlled dialogs return focus", () => {
  it("restores focus to the Remove button when the AlertDialog closes", async () => {
    render(
      <SeriesRelatedSection
        seriesSlug="saga-vol-2"
        seriesId="s2"
        coverWidth={160}
      />,
    );
    const remove = screen.getByRole("button", {
      name: "Remove continued by Saga Vol 3",
    });
    remove.focus();
    await act(async () => {
      fireEvent.click(remove);
    });
    expect(
      screen.getByRole("alertdialog", { name: "Remove relationship?" }),
    ).toBeTruthy();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    });
    // Radix hands focus back asynchronously (after the close transition),
    // so poll instead of asserting on the same tick.
    await waitFor(() => expect(document.activeElement).toBe(remove));
  });
});
