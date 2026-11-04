/**
 * Drag-to-turn physics (`lib/reader/drag-turn.ts`): the flick / distance
 * decision, rubber-band resistance at the ends, velocity sampling and
 * the settle duration.
 */
import { describe, expect, it } from "vitest";
import {
  decideDragTurn,
  dragIntent,
  dragOffset,
  releaseVelocity,
  rubberBand,
  turnDurationMs,
} from "@/lib/reader/drag-turn";

const W = 1000;
const base = {
  width: W,
  direction: "ltr" as const,
  canNext: true,
  canPrev: true,
};

describe("decideDragTurn", () => {
  it("springs back from a slow, short drag", () => {
    expect(decideDragTurn({ ...base, dx: -120, velocity: -0.1 })).toBe("back");
    expect(decideDragTurn({ ...base, dx: 200, velocity: 0 })).toBe("back");
  });

  it("turns past 30% of the width even when released at rest", () => {
    expect(decideDragTurn({ ...base, dx: -301, velocity: 0 })).toBe("next");
    expect(decideDragTurn({ ...base, dx: 301, velocity: 0 })).toBe("prev");
  });

  it("turns on a flick in the drag direction", () => {
    expect(decideDragTurn({ ...base, dx: -40, velocity: -0.8 })).toBe("next");
    expect(decideDragTurn({ ...base, dx: 40, velocity: 0.8 })).toBe("prev");
  });

  it("ignores a flick that barely moved", () => {
    expect(decideDragTurn({ ...base, dx: -10, velocity: -2 })).toBe("back");
  });

  it("springs back when the release is thrown the other way", () => {
    expect(decideDragTurn({ ...base, dx: -400, velocity: 0.9 })).toBe("back");
  });

  it("maps directions in RTL", () => {
    const rtl = { ...base, direction: "rtl" as const };
    expect(decideDragTurn({ ...rtl, dx: 400, velocity: 0 })).toBe("next");
    expect(decideDragTurn({ ...rtl, dx: -400, velocity: 0 })).toBe("prev");
  });

  it("asks for the end card on the last page, springs back on the first", () => {
    expect(
      decideDragTurn({ ...base, canNext: false, dx: -400, velocity: 0 }),
    ).toBe("end");
    expect(
      decideDragTurn({ ...base, canPrev: false, dx: 400, velocity: 0 }),
    ).toBe("back");
  });
});

describe("rubberBand / dragOffset", () => {
  it("is zero at rest and keeps the drag's sign", () => {
    expect(rubberBand(0, W)).toBe(0);
    expect(rubberBand(100, W)).toBeGreaterThan(0);
    expect(rubberBand(-100, W)).toBeLessThan(0);
    expect(rubberBand(-100, W)).toBeCloseTo(-rubberBand(100, W));
  });

  it("resists: always less than the pointer travel, never past half width", () => {
    for (const dx of [10, 100, 500, 2000, 100000]) {
      const r = rubberBand(dx, W);
      expect(r).toBeLessThan(dx);
      expect(r).toBeLessThan(W / 2);
    }
    expect(rubberBand(2000, W)).toBeGreaterThan(rubberBand(500, W));
  });

  it("follows 1:1 when not blocked, clamped to one width", () => {
    expect(dragOffset(-250, W, false)).toBe(-250);
    expect(dragOffset(1500, W, false)).toBe(W);
    expect(dragOffset(250, W, true)).toBe(rubberBand(250, W));
  });

  it("dragIntent reads left drags as forward in LTR, backward in RTL", () => {
    expect(dragIntent(-5, "ltr")).toBe("next");
    expect(dragIntent(5, "ltr")).toBe("prev");
    expect(dragIntent(5, "rtl")).toBe("next");
    expect(dragIntent(-5, "rtl")).toBe("prev");
  });
});

describe("releaseVelocity", () => {
  it("measures the trailing window only", () => {
    const samples = [
      { x: 0, t: 0 },
      { x: -10, t: 400 },
      { x: -60, t: 450 },
      { x: -110, t: 500 },
    ];
    // Last 100ms: -10 → -110 over 100ms.
    expect(releaseVelocity(samples, 500)).toBeCloseTo(-1);
  });

  it("is zero when the pointer rested before release", () => {
    const samples = [
      { x: 0, t: 0 },
      { x: -200, t: 50 },
    ];
    expect(releaseVelocity(samples, 400)).toBe(0);
  });

  it("is zero with fewer than two samples", () => {
    expect(releaseVelocity([], 0)).toBe(0);
    expect(releaseVelocity([{ x: 5, t: 0 }], 0)).toBe(0);
  });
});

describe("turnDurationMs", () => {
  it("matches a fast flick's speed, bounded below", () => {
    expect(turnDurationMs(800, 4, W)).toBe(200);
    expect(turnDurationMs(100, 10, W)).toBe(90);
  });

  it("never settles slower than a full width in ~300ms", () => {
    expect(turnDurationMs(700, 0, W)).toBe(210);
    expect(turnDurationMs(5000, 0, W)).toBe(320);
  });
});
