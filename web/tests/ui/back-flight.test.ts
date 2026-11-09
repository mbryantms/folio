// @vitest-environment jsdom
/**
 * The in-app Back flight (lib/ui/back-flight.ts): a fixed copy of the header
 * cover laid over the page at the click.
 *
 * Regression: only `landBackFlight` ever removed the copy, and nothing called
 * it when Back landed on the reader (no `BackFlightLayer` there). The copy
 * stayed on `document.body`, over every later page, until the next Back.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { landBackFlight, startBackFlight } from "@/lib/ui/back-flight";

const NAME = "issue-cover-a-1";

type FakeAnimation = {
  keyframes: Keyframe[];
  onfinish: (() => void) | null;
  oncancel: (() => void) | null;
};
let animations: FakeAnimation[] = [];

const box = (left: number, top: number, width: number, height: number) =>
  ({
    left,
    top,
    width,
    height,
    bottom: top + height,
    right: left + width,
  }) as DOMRect;

/** The page the flight leaves: a header cover, on screen. */
function mountHero() {
  const hero = document.createElement("div");
  hero.dataset.coverHero = NAME;
  const img = document.createElement("img");
  img.src = "/cover.jpg";
  img.getBoundingClientRect = () => box(10, 10, 200, 300);
  hero.append(img);
  document.body.append(hero);
}

/** The page it returns to: the card carrying the same cover name. */
function mountCard() {
  const card = document.createElement("div");
  card.dataset.coverLanding = NAME;
  const img = document.createElement("img");
  img.getBoundingClientRect = () => box(400, 100, 100, 150);
  card.append(img);
  document.body.append(card);
  return img;
}

/** The floating copy: the only image directly on `<body>`. */
const copies = () =>
  Array.from(document.body.children).filter((el) => el.tagName === "IMG");

const fades = () => animations.filter((a) => "opacity" in a.keyframes[0]!);
const flights = () => animations.filter((a) => "boxShadow" in a.keyframes[0]!);

beforeEach(() => {
  vi.useFakeTimers();
  vi.stubGlobal("CSS", { escape: (s: string) => s });
  animations = [];
  HTMLElement.prototype.animate = function (keyframes: Keyframe[]) {
    const anim: FakeAnimation = { keyframes, onfinish: null, oncancel: null };
    animations.push(anim);
    return anim as unknown as Animation;
  } as unknown as typeof HTMLElement.prototype.animate;
  mountHero();
});

afterEach(() => {
  // Drops a flight still waiting, so one test can't leak into the next.
  document.body.replaceChildren();
  startBackFlight();
  vi.runOnlyPendingTimers();
  vi.useRealTimers();
  vi.unstubAllGlobals();
  document.body.replaceChildren();
});

describe("Back flight", () => {
  it("lays one copy of the header cover over the page", () => {
    startBackFlight();
    expect(copies()).toHaveLength(1);
    expect((copies()[0] as HTMLElement).style.position).toBe("fixed");
  });

  it("removes the copy by itself when nothing ever lands it", () => {
    startBackFlight();

    vi.advanceTimersByTime(699);
    expect(fades()).toHaveLength(0);
    expect(copies()).toHaveLength(1);

    vi.advanceTimersByTime(1);
    expect(fades()).toHaveLength(1);

    fades()[0]!.onfinish?.();
    expect(copies()).toHaveLength(0);
  });

  it("removes the copy when Back doesn't change the pathname", () => {
    startBackFlight();
    landBackFlight(location.pathname);

    vi.advanceTimersByTime(700);
    fades()[0]!.onfinish?.();
    expect(copies()).toHaveLength(0);
  });

  it("flies to the card and never fades once it has taken off", () => {
    const card = mountCard();
    startBackFlight();
    landBackFlight("/series/a");

    vi.advanceTimersByTime(100);
    expect(flights()).toHaveLength(1);
    expect(card.style.visibility).toBe("hidden");

    vi.advanceTimersByTime(5000);
    expect(fades()).toHaveLength(0);
    expect(flights()).toHaveLength(1);

    flights()[0]!.onfinish?.();
    expect(copies()).toHaveLength(0);
    expect(card.style.visibility).toBe("");
  });

  it("fades once and stops looking when no card is on screen", () => {
    startBackFlight();
    landBackFlight("/series/a");

    vi.advanceTimersByTime(5000);
    expect(fades()).toHaveLength(1);
    expect(flights()).toHaveLength(0);

    // A card turning up after the fade began gets no second animation.
    mountCard();
    vi.advanceTimersByTime(5000);
    expect(animations).toHaveLength(1);
  });

  it("replaces a flight still waiting when Back is clicked again", () => {
    startBackFlight();
    vi.advanceTimersByTime(400);
    startBackFlight();
    expect(copies()).toHaveLength(1);

    // The first flight's expiry is gone: only the second one fades, 700 ms
    // after its own click.
    vi.advanceTimersByTime(699);
    expect(fades()).toHaveLength(0);
    vi.advanceTimersByTime(1);
    expect(fades()).toHaveLength(1);
  });
});
