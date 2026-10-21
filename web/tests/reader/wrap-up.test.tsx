// @vitest-environment jsdom
/**
 * End-of-issue Wrap up: the bottom strip on the Up Next pane, the pane
 * flip, the `1`–`5` rating keys, and the closing-note editor's dirty /
 * clear rules. Rendering with Testing Library so the pane switch and
 * key handling run for real; the card has no QueryClient or router
 * dependency (the reader hands it plain values + callbacks).
 */
import { act, fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { EndOfIssueCard } from "@/app/[locale]/read/[seriesSlug]/[issueSlug]/EndOfIssueCard";
import { ClosingNoteEditor } from "@/components/library/ClosingNoteEditor";
import type { IssueSummaryView, NextUpView } from "@/lib/api/types";
import {
  markerBreakdown,
  markerTotalLabel,
  ratingFromKey,
  summarizeMarkers,
  type WrapUpProps,
} from "@/lib/reader/wrap-up";

function issue(): IssueSummaryView {
  return {
    id: "i2",
    slug: "issue-2",
    series_id: "s1",
    series_slug: "saga",
    series_name: "Saga",
    title: null,
    number: "12",
    sort_number: 12,
    year: 2013,
    page_count: 22,
    state: "active",
    cover_url: null,
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
  } as unknown as IssueSummaryView;
}

const nextUp: NextUpView = {
  source: "series",
  target: issue(),
} as unknown as NextUpView;

function wrapUp(overrides: Partial<WrapUpProps> = {}): WrapUpProps {
  return {
    issueLabel: "Saga #11",
    rating: null,
    onRate: vi.fn(),
    note: null,
    onSaveNote: vi.fn(),
    markerCounts: {
      total: 3,
      bookmarks: 0,
      notes: 1,
      favorites: 0,
      highlights: 2,
    },
    onReviewMarkers: vi.fn(),
    ...overrides,
  };
}

function renderCard(props: Partial<Parameters<typeof EndOfIssueCard>[0]> = {}) {
  return render(
    <EndOfIssueCard
      open
      data={nextUp}
      isLoading={false}
      direction="ltr"
      exitUrl="/series/saga/issues/issue-11"
      onContinue={() => {}}
      onDismiss={() => {}}
      {...props}
    />,
  );
}

describe("wrap-up helpers", () => {
  it("tallies markers by kind and words the breakdown", () => {
    const c = summarizeMarkers([
      { kind: "highlight" },
      { kind: "highlight" },
      { kind: "note" },
      { kind: "bookmark" },
    ]);
    expect(c).toEqual({
      total: 4,
      bookmarks: 1,
      notes: 1,
      favorites: 0,
      highlights: 2,
    });
    expect(markerTotalLabel(c)).toBe("4 markers");
    expect(markerBreakdown(c)).toBe("2 highlights, 1 note, 1 bookmark");
    expect(markerTotalLabel(summarizeMarkers([]))).toBe("No markers");
    expect(markerBreakdown(summarizeMarkers(undefined))).toBe("");
  });

  it("maps 1–5 to a rating and a repeat press to clear; ignores 0", () => {
    expect(ratingFromKey("3", null)).toBe(3);
    expect(ratingFromKey("3", 3)).toBeNull();
    expect(ratingFromKey("0", 2)).toBeUndefined();
    expect(ratingFromKey("n", 2)).toBeUndefined();
  });
});

describe("<EndOfIssueCard> wrap up", () => {
  it("renders exactly as before when no wrapUp is given", () => {
    const { container } = renderCard();
    expect(container.querySelector("[data-wrap-up-strip]")).toBeNull();
    expect(screen.queryByRole("button", { name: /wrap up/i })).toBeNull();
    expect(screen.getByRole("button", { name: "Read" })).toBeTruthy();
  });

  it("keeps the Up Next flow and adds the strip at the bottom", () => {
    const { container } = renderCard({ wrapUp: wrapUp() });
    const card = container.querySelector('[role="dialog"]')!;
    const strip = card.querySelector("[data-wrap-up-strip]")!;
    // Last child of the card: below Read and the issue-page links.
    expect(card.lastElementChild).toBe(strip);
    expect(screen.getByRole("button", { name: "Read" })).toBeTruthy();
    expect(screen.getByText("Rate")).toBeTruthy();
    expect(screen.getByLabelText("3 markers in this issue")).toBeTruthy();
    // Neither the star group nor the Wrap up button can shrink under
    // its neighbour (the overlap the mockup review caught).
    expect(strip.querySelector("[role=radiogroup]")?.className).toMatch(
      /\bshrink-0\b/,
    );
    expect(strip.querySelector(":scope > button")?.className).toMatch(
      /\bshrink-0\b/,
    );
  });

  it("shows the value instead of the prompt once rated", () => {
    renderCard({ wrapUp: wrapUp({ rating: 4 }) });
    expect(screen.getByText("4/5")).toBeTruthy();
    expect(screen.queryByText("Rate")).toBeNull();
  });

  it("flips to the Wrap up pane and back without losing Read", () => {
    const onReview = vi.fn();
    renderCard({ wrapUp: wrapUp({ onReviewMarkers: onReview }) });
    fireEvent.click(screen.getByRole("button", { name: /wrap up/i }));
    expect(screen.getByText("Saga #11")).toBeTruthy();
    expect(screen.getByLabelText("Closing note")).toBeTruthy();
    expect(screen.getByText("3 markers")).toBeTruthy();
    expect(screen.getByText("2 highlights, 1 note")).toBeTruthy();
    // Read stays one tap away on the second pane.
    expect(screen.getByRole("button", { name: "Read Saga #12" })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Review" }));
    expect(onReview).toHaveBeenCalledTimes(1);
    // Two ways back: the header arrow and the footer text link.
    const backs = screen.getAllByRole("button", { name: "Back to Up Next" });
    expect(backs).toHaveLength(2);
    fireEvent.click(backs[0]!);
    expect(screen.getByRole("button", { name: "Read" })).toBeTruthy();
  });

  it("resets to Up Next when the card closes", () => {
    const { rerender } = renderCard({ wrapUp: wrapUp() });
    fireEvent.click(screen.getByRole("button", { name: /wrap up/i }));
    expect(screen.getByLabelText("Closing note")).toBeTruthy();
    rerender(
      <EndOfIssueCard
        open={false}
        data={nextUp}
        isLoading={false}
        direction="ltr"
        exitUrl="/x"
        onContinue={() => {}}
        onDismiss={() => {}}
        wrapUp={wrapUp()}
      />,
    );
    rerender(
      <EndOfIssueCard
        open
        data={nextUp}
        isLoading={false}
        direction="ltr"
        exitUrl="/x"
        onContinue={() => {}}
        onDismiss={() => {}}
        wrapUp={wrapUp()}
      />,
    );
    expect(screen.queryByLabelText("Closing note")).toBeNull();
    expect(screen.getByRole("button", { name: "Read" })).toBeTruthy();
  });

  it("rates with 1–5 while open, not while typing, not while closed", () => {
    const onRate = vi.fn();
    const { rerender } = renderCard({ wrapUp: wrapUp({ onRate, rating: 2 }) });
    act(() => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "4" }));
    });
    expect(onRate).toHaveBeenLastCalledWith(4);
    // Repeat press on the current value clears.
    act(() => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "2" }));
    });
    expect(onRate).toHaveBeenLastCalledWith(null);
    // `0` is the reader's zoom-reset — untouched.
    onRate.mockClear();
    act(() => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "0" }));
    });
    expect(onRate).not.toHaveBeenCalled();
    // Typing in the closing note never rates.
    fireEvent.click(screen.getByRole("button", { name: /wrap up/i }));
    const textarea = screen.getByLabelText("Closing note");
    fireEvent.keyDown(textarea, { key: "5", bubbles: true });
    expect(onRate).not.toHaveBeenCalled();
    // Closed card: keys fall through to the reader.
    rerender(
      <EndOfIssueCard
        open={false}
        data={nextUp}
        isLoading={false}
        direction="ltr"
        exitUrl="/x"
        onContinue={() => {}}
        onDismiss={() => {}}
        wrapUp={wrapUp({ onRate })}
      />,
    );
    act(() => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "3" }));
    });
    expect(onRate).not.toHaveBeenCalled();
  });
});

describe("<ClosingNoteEditor>", () => {
  it("saves only when dirty, trims, and clears with an emptied draft", () => {
    const onSave = vi.fn();
    const { rerender } = render(
      <ClosingNoteEditor value={null} onSave={onSave} />,
    );
    const save = screen.getByRole("button", { name: "Save note" });
    expect(save).toHaveProperty("disabled", true);
    fireEvent.change(screen.getByLabelText("Closing note"), {
      target: { value: "  Loved the last act.  " },
    });
    expect(save).toHaveProperty("disabled", false);
    fireEvent.click(save);
    expect(onSave).toHaveBeenCalledWith("Loved the last act.");

    rerender(<ClosingNoteEditor value="Loved the last act." onSave={onSave} />);
    fireEvent.change(screen.getByLabelText("Closing note"), {
      target: { value: "   " },
    });
    const clear = screen.getByRole("button", { name: "Clear note" });
    fireEvent.click(clear);
    expect(onSave).toHaveBeenLastCalledWith(null);
  });

  it("page variant shows the note with Edit, or an add link when empty", () => {
    const { rerender } = render(
      <ClosingNoteEditor variant="page" value={null} onSave={() => {}} />,
    );
    expect(
      screen.getByRole("button", { name: "Add a closing note" }),
    ).toBeTruthy();
    expect(screen.queryByLabelText("Your closing note")).toBeNull();
    rerender(
      <ClosingNoteEditor variant="page" value="Great arc." onSave={() => {}} />,
    );
    expect(screen.getByText("Great arc.")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    expect(screen.getByLabelText("Your closing note")).toBeTruthy();
  });
});
