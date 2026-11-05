// @vitest-environment jsdom
/**
 * Library grid filter chips (flair pre-merge polish): a removed chip stays
 * mounted briefly as an inert, aria-hidden "leaving" chip so it can fade
 * out, then goes; under reduced motion it goes at once.
 */
import { act, render, screen } from "@testing-library/react";
import { createElement } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const motion = { reduced: false };
vi.mock("@/lib/visual-effects", () => ({
  useReducedMotion: () => motion.reduced,
}));

import { ActiveChips } from "@/components/library/ActiveChips";
import { CREDIT_ROLES } from "@/components/library/library-grid-filters";

const noop = () => undefined;
function props(genres: string[]) {
  return {
    status: "any" as const,
    metadataCompleteness: undefined,
    providerMatch: undefined,
    providerSource: undefined,
    readStatus: [],
    yearFrom: "",
    yearTo: "",
    ratingRange: null,
    publishers: [],
    languages: [],
    ageRatings: [],
    genres,
    tags: [],
    credits: Object.fromEntries(CREDIT_ROLES.map((c) => [c.key, []])),
    anyCredits: [],
    characters: [],
    teams: [],
    locations: [],
    onClearStatus: noop,
    onClearMetadataCompleteness: noop,
    onClearProviderMatch: noop,
    onRemoveReadStatus: noop,
    onClearYear: noop,
    onClearRating: noop,
    onRemovePublisher: noop,
    onRemoveLanguage: noop,
    onRemoveAgeRating: noop,
    onRemoveGenre: noop,
    onRemoveTag: noop,
    onRemoveCredit: noop,
    onRemoveAnyCredit: noop,
    onRemoveCharacter: noop,
    onRemoveTeam: noop,
    onRemoveLocation: noop,
  } as unknown as Parameters<typeof ActiveChips>[0];
}

describe("ActiveChips presence", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    motion.reduced = false;
  });
  afterEach(() => vi.useRealTimers());

  it("renders nothing with no active facets", () => {
    const { container } = render(createElement(ActiveChips, props([])));
    expect(container.innerHTML).toBe("");
  });

  it("keeps a removed chip as an inert leaving chip, then drops it", () => {
    const { container, rerender } = render(
      createElement(ActiveChips, props(["Horror", "Noir"])),
    );
    rerender(createElement(ActiveChips, props(["Noir"])));
    const leaving = container.querySelector(".filter-chip-leave");
    expect(leaving?.textContent).toContain("Genre: Horror");
    expect(leaving?.getAttribute("aria-hidden")).toBe("true");
    // Order holds: the leaving chip sits where it was, before "Noir".
    expect(container.textContent?.indexOf("Horror")).toBeLessThan(
      container.textContent?.indexOf("Noir") ?? -1,
    );
    expect(
      screen.queryByRole("button", { name: "Remove Genre: Horror" }),
    ).toBeNull();
    act(() => {
      vi.advanceTimersByTime(300);
    });
    expect(container.querySelector(".filter-chip-leave")).toBeNull();
    expect(container.textContent).toContain("Genre: Noir");
  });

  it("the last chip still animates out before the row unmounts", () => {
    const { container, rerender } = render(
      createElement(ActiveChips, props(["Horror"])),
    );
    rerender(createElement(ActiveChips, props([])));
    expect(container.querySelector(".filter-chip-leave")).not.toBeNull();
    act(() => {
      vi.advanceTimersByTime(300);
    });
    expect(container.innerHTML).toBe("");
  });

  it("drops removed chips at once under reduced motion", () => {
    motion.reduced = true;
    const { container, rerender } = render(
      createElement(ActiveChips, props(["Horror"])),
    );
    rerender(createElement(ActiveChips, props([])));
    expect(container.innerHTML).toBe("");
  });
});
