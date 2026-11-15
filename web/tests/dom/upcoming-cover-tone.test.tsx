// @vitest-environment jsdom
/**
 * An upcoming release's card takes its cover's colour from the server
 * (WP-9.6): Metron's image host sends no CORS headers, so the browser can
 * never sample the picture itself. The placeholder box shows the average
 * colour and the card gets `--cover-tint` (the hover wash and glow in
 * globals.css key off that property).
 */
import { cleanup, render } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("@/lib/visual-effects", () => ({
  useVisualEffect: () => false,
  useReducedMotion: () => true,
}));

import { UpcomingCard } from "@/components/upcoming/UpcomingCard";
import type { UpcomingEntryView } from "@/lib/api/types";

function entry(over: Partial<UpcomingEntryView>): UpcomingEntryView {
  return {
    key: "m1",
    number: "7",
    series_name: "Saga",
    publisher: "Image",
    release_date: "2026-11-18",
    stage: "announced",
    cover_url: "https://static.metron.cloud/media/issue/1.jpg",
    ...over,
  } as UpcomingEntryView;
}

afterEach(() => {
  cleanup();
  localStorage.clear();
});

describe("upcoming card cover colour", () => {
  it("uses the colours the server read", () => {
    const { container } = render(
      <UpcomingCard
        entry={entry({ cover_tone: "c8281ef0a020" })}
        onOpen={() => {}}
      />,
    );
    const card = container.querySelector<HTMLElement>("[data-card]")!;
    const box = container.querySelector<HTMLElement>(".cover-box")!;
    // Average colour as the placeholder, vibrant colour as the tint.
    expect(box.style.backgroundColor).toBe("rgb(200, 40, 30)");
    expect(card.style.getPropertyValue("--cover-tint")).toBe("rgb(240 160 32)");
    // Inside the shared frame, so the glow's `.cover-box` selector lands.
    expect(box.closest("[data-cover-frame]")).toBeTruthy();
  });

  it("has no colour until the server has read the cover", () => {
    const { container } = render(
      <UpcomingCard entry={entry({})} onOpen={() => {}} />,
    );
    const card = container.querySelector<HTMLElement>("[data-card]")!;
    expect(card.style.getPropertyValue("--cover-tint")).toBe("");
    expect(
      container.querySelector<HTMLElement>(".cover-box")!.style.backgroundColor,
    ).toBe("");
  });

  it("ignores a value that is not twelve hex digits", () => {
    const { container } = render(
      <UpcomingCard
        entry={entry({ cover_tone: "red; background: url(x)" })}
        onOpen={() => {}}
      />,
    );
    const card = container.querySelector<HTMLElement>("[data-card]")!;
    expect(card.style.getPropertyValue("--cover-tint")).toBe("");
  });
});
