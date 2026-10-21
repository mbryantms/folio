// @vitest-environment jsdom
/**
 * `/bookmarks` on the unified feed: closing notes as text rows, the
 * grouped / flat splitters, the filter-aware header copy, and the
 * clamp + More / Less on a long closing note.
 */
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));
vi.mock("@/lib/api/mutations", () => ({
  useSetIssueNote: () => ({ mutate: vi.fn(), isPending: false }),
}));

import {
  ClosingNoteRow,
  closingNoteIssueLabel,
  needsClamp,
} from "@/components/markers/ClosingNoteRow";
import {
  groupEntriesBySeries,
  headerDescription,
  splitIntoRuns,
} from "@/components/markers/MarkersList";
import type {
  BookmarkEntryView,
  ClosingNoteEntryView,
  MarkerView,
} from "@/lib/api/types";

function marker(overrides: Partial<MarkerView>): MarkerView {
  return {
    id: "m",
    user_id: "u",
    series_id: "s-saga",
    series_name: "Saga",
    series_slug: "saga",
    issue_id: "i1",
    issue_slug: "1",
    issue_number: "1",
    page_index: 0,
    kind: "bookmark",
    is_favorite: false,
    tags: [],
    region: null,
    selection: null,
    body: null,
    color: null,
    hidden_from_log: false,
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
    ...overrides,
  } as MarkerView;
}

function note(overrides: Partial<ClosingNoteEntryView>): ClosingNoteEntryView {
  return {
    issue_id: "i2",
    series_id: "s-saga",
    series_name: "Saga",
    series_slug: "saga",
    issue_slug: "2",
    issue_title: null,
    issue_number: "2",
    cover_url: null,
    rating: 4,
    body: "The arc closer.",
    created_at: "2026-01-02T00:00:00Z",
    updated_at: "2026-01-02T00:00:00Z",
    ...overrides,
  };
}

const entries: BookmarkEntryView[] = [
  {
    type: "marker",
    marker: marker({
      id: "a",
      series_id: "s-alpha",
      series_name: "Alpha Flight",
    }),
  },
  { type: "closing_note", note: note({ issue_id: "i2" }) },
  { type: "marker", marker: marker({ id: "b", page_index: 3 }) },
  { type: "marker", marker: marker({ id: "c", page_index: 5 }) },
  {
    type: "closing_note",
    note: note({
      issue_id: "i9",
      issue_number: "9",
      series_id: "s-alpha",
      series_name: "Alpha Flight",
    }),
  },
];

describe("feed splitters", () => {
  it("groups by series in first-appearance order, notes apart from markers", () => {
    const groups = groupEntriesBySeries(entries);
    expect(groups.map((g) => g.label)).toEqual(["Alpha Flight", "Saga"]);
    expect(groups[0]!.markers.map((m) => m.id)).toEqual(["a"]);
    expect(groups[0]!.notes.map((n) => n.issue_id)).toEqual(["i9"]);
    expect(groups[1]!.notes.map((n) => n.issue_id)).toEqual(["i2"]);
    expect(groups[1]!.markers.map((m) => m.id)).toEqual(["b", "c"]);
  });

  it("flat view keeps server order: packed runs broken by note rows", () => {
    const runs = splitIntoRuns(entries);
    expect(
      runs.map((r) =>
        r.type === "note"
          ? `note:${r.note.issue_id}`
          : `run:${r.markers.map((m) => m.id).join("")}`,
      ),
    ).toEqual(["run:a", "note:i2", "run:bc", "note:i9"]);
  });
});

describe("headerDescription", () => {
  it("states the total, or Showing N of M once filtered", () => {
    expect(
      headerDescription({ all: undefined, shown: undefined, filtered: false }),
    ).toMatch(/closing note/);
    expect(headerDescription({ all: 41, shown: 41, filtered: false })).toMatch(
      /^41 saved/,
    );
    expect(
      headerDescription({
        all: 41,
        shown: 14,
        filtered: true,
        seriesName: "Saga",
      }),
    ).toBe("Showing 14 of 41 saved — filtered to Saga.");
    expect(headerDescription({ all: 41, shown: 9, filtered: true })).toBe(
      "Showing 9 of 41 saved.",
    );
  });
});

describe("<ClosingNoteRow>", () => {
  it("labels the issue and clamps only long or multi-line bodies", () => {
    expect(closingNoteIssueLabel(note({}))).toBe("Saga · #2");
    expect(
      closingNoteIssueLabel(
        note({ issue_number: null, issue_title: "Annual" }),
      ),
    ).toBe("Saga · Annual");
    expect(needsClamp("Short.")).toBe(false);
    expect(needsClamp("One.\n\nTwo.")).toBe(true);
    expect(needsClamp("x".repeat(300))).toBe(true);
  });

  it("renders as a text row with rating, date and an in-place More / Less", () => {
    const long = Array.from(
      { length: 6 },
      (_, i) => `Paragraph ${i + 1}.`,
    ).join("\n\n");
    const { container } = render(
      <ClosingNoteRow note={note({ body: long })} />,
    );
    expect(container.querySelector("[data-closing-note-row]")).toBeTruthy();
    expect(screen.getByText("Closing note")).toBeTruthy();
    // Label appears in the row and as the cover's text fallback.
    expect(screen.getAllByText("Saga · #2").length).toBeGreaterThan(0);
    expect(screen.getByLabelText(/Your rating of Saga · #2/)).toBeTruthy();
    const body = container.querySelector("p")!;
    expect(body.className).toMatch(/\bline-clamp-4\b/);
    const more = screen.getByRole("button", { name: "More" });
    fireEvent.click(more);
    expect(body.className).not.toMatch(/\bline-clamp-4\b/);
    expect(screen.getByRole("button", { name: "Less" })).toBeTruthy();
    // No picture tile: the row is text first, with a 56px cover slot.
    expect(screen.getByRole("link", { name: "Open Saga · #2" })).toBeTruthy();
  });

  it("renders a short note without the toggle", () => {
    render(<ClosingNoteRow note={note({ body: "Loved it." })} />);
    expect(screen.getByText("Loved it.")).toBeTruthy();
    expect(screen.queryByRole("button", { name: /More|Less/ })).toBeNull();
  });
});
