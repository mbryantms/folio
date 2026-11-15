/**
 * The cover's colour is in the server-rendered markup (WP-9.7): a device
 * that has never loaded a cover still gets its placeholder colour and
 * the card's hover tint on first paint, from the pair the server sent
 * (`cover_tone`). Static markup on purpose: no effects run, so anything
 * asserted here is what the server sends.
 */
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";

vi.mock("@/lib/visual-effects", () => ({
  useVisualEffect: () => false,
  useReducedMotion: () => true,
}));

import { Cover } from "@/components/Cover";
import { UpcomingCard } from "@/components/upcoming/UpcomingCard";
import type { UpcomingEntryView } from "@/lib/api/types";
import {
  coverPlaceholderStyle,
  coverTintStyle,
  decodeTone,
} from "@/lib/ui/cover-tone-style";

const TONE = "c8281ef0a020"; // average 200,40,30; vibrant 240,160,32

describe("the colour pair", () => {
  it("decodes to two rgb triplets", () => {
    expect(decodeTone(TONE)).toEqual({
      average: "200 40 30",
      vibrant: "240 160 32",
    });
  });

  it("rejects anything that is not twelve lowercase hex digits", () => {
    for (const bad of [
      null,
      undefined,
      "",
      "c8281e",
      "C8281EF0A020",
      "c8281ef0a02g",
      "red;background:url(x)",
    ]) {
      expect(decodeTone(bad)).toBeNull();
      expect(coverTintStyle(bad)).toBeUndefined();
      expect(coverPlaceholderStyle(bad)).toBeUndefined();
    }
  });

  it("becomes the card's tint and the box's placeholder colour", () => {
    expect(coverTintStyle(TONE)).toEqual({
      "--cover-tint": "rgb(240 160 32)",
    });
    expect(coverPlaceholderStyle(TONE)).toEqual({
      backgroundColor: "rgb(200 40 30)",
    });
  });
});

describe("first paint", () => {
  it("a cover's box carries its average colour in the markup", () => {
    const html = renderToStaticMarkup(
      <Cover src="/issues/i1/pages/0/thumb" alt="Saga #1" tone={TONE} />,
    );
    expect(html).toMatch(
      /class="cover-box[^"]*" style="background-color:rgb\(200 40 30\)"/,
    );
  });

  it("a cover without a pair has no inline colour", () => {
    const html = renderToStaticMarkup(
      <Cover src="/issues/i1/pages/0/thumb" alt="Saga #1" />,
    );
    expect(html).not.toContain("background-color");
  });

  it("a card carries its tint in the markup", () => {
    const entry = {
      key: "m1",
      number: "7",
      series_name: "Saga",
      publisher: "Image",
      release_date: "2026-11-18",
      stage: "announced",
      cover_url: "https://static.metron.cloud/media/issue/1.jpg",
      cover_tone: TONE,
    } as UpcomingEntryView;
    const html = renderToStaticMarkup(
      <UpcomingCard entry={entry} onOpen={() => {}} />,
    );
    expect(html).toMatch(
      /<button[^>]*data-card=""[^>]*style="--cover-tint:rgb\(240 160 32\)"/,
    );
  });
});
