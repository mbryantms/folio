// @vitest-environment jsdom
/**
 * Year in review story (folio-flair phase 4): slides advance by button,
 * tap zone and arrow keys, the progress segments jump, Escape closes, and
 * an empty year collapses to one slide with other years offered.
 */
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

// Effects read the account (`useMe`); reduced motion keeps the story still.
vi.mock("@/lib/visual-effects", () => ({
  useVisualEffect: () => false,
  useReducedMotion: () => true,
}));

import { YearStory, type YearModel } from "@/components/log/year/YearStory";
import { monthBars } from "@/lib/log/year";

const days = [
  { date: "2026-03-01", pages: 40 },
  { date: "2026-03-02", pages: 60 },
  { date: "2026-03-03", pages: 20 },
];

const model: YearModel = {
  year: 2026,
  inProgress: false,
  pages: 120,
  issues: 6,
  activeMs: 3 * 3_600_000,
  sessions: 7,
  daysActive: 3,
  daysInScope: 365,
  months: monthBars(days, 2026),
  busiest: 2,
  bigDay: days[1]!,
  run: { start: "2026-03-01", end: "2026-03-03", length: 3 },
  rhythm: null,
  top: [
    {
      series_id: "s1",
      name: "Saga",
      active_ms: 2 * 3_600_000,
      sessions: 4,
      cover: null,
      finished: 3,
    },
  ],
  mosaic: { covers: [], total: 6, more: false },
  coversLoading: false,
};

function setup(m: YearModel = model, years = [2026, 2025]) {
  const onClose = vi.fn();
  const onPickYear = vi.fn();
  render(
    <YearStory
      year={m.year}
      model={m}
      loading={false}
      error={false}
      years={years}
      onPickYear={onPickYear}
      onClose={onClose}
    />,
  );
  return { onClose, onPickYear };
}

const current = () =>
  document
    .querySelector("[aria-roledescription='slide']")
    ?.getAttribute("aria-label");

describe("YearStory", () => {
  it("opens on the intro, focused, with a segment per slide", () => {
    setup();
    expect(current()).toBe("1 of 7: Your year");
    expect(screen.getAllByText("2026").length).toBeGreaterThan(0);
    expect(document.activeElement?.getAttribute("aria-roledescription")).toBe(
      "story",
    );
    const segments = screen.getAllByRole("button", { name: /^Slide \d/ });
    expect(segments).toHaveLength(7);
    expect(segments[0]!.getAttribute("aria-current")).toBe("step");
    // Reduced motion: no auto-advance, so no pause control.
    expect(screen.queryByRole("button", { name: /pause story/i })).toBeNull();
  });

  it("moves with the footer, keys and segments, and closes on Escape", () => {
    const { onClose } = setup();
    const [, footerNext] = screen.getAllByRole("button", {
      name: "Next slide",
    });
    fireEvent.click(footerNext!);
    expect(current()).toBe("2 of 7: Pages and time");
    fireEvent.keyDown(window, { key: "ArrowRight" });
    expect(current()).toBe("3 of 7: Busiest month");
    expect(screen.getByText("March")).toBeTruthy();
    fireEvent.keyDown(window, { key: "ArrowLeft" });
    expect(current()).toBe("2 of 7: Pages and time");
    fireEvent.click(screen.getByRole("button", { name: /^Slide 7/ }));
    expect(current()).toBe("7 of 7: Summary");
    fireEvent.click(screen.getByRole("button", { name: /watch again/i }));
    expect(current()).toBe("1 of 7: Your year");
    fireEvent.keyDown(window, { key: "Escape" });
    expect(onClose).toHaveBeenCalledOnce();
  });

  it("offers the year picker over years with reading", () => {
    const { onPickYear } = setup();
    fireEvent.change(screen.getByLabelText("Year"), {
      target: { value: "2025" },
    });
    expect(onPickYear).toHaveBeenCalledWith(2025);
  });

  it("collapses an empty year to one slide with other years", () => {
    const { onPickYear } = setup(
      {
        ...model,
        pages: 0,
        issues: 0,
        sessions: 0,
        daysActive: 0,
        busiest: -1,
        run: null,
        top: [],
        mosaic: { covers: [], total: 0, more: false },
      },
      [2026, 2024],
    );
    expect(current()).toBe("1 of 1: No reading");
    expect(screen.getByText("No reading this year")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Next slide" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "See 2024" }));
    expect(onPickYear).toHaveBeenCalledWith(2024);
  });
});
