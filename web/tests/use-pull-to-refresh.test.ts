/**
 * Pure resistance-curve math for `usePullToRefresh`. The shape of
 * the curve matters: linear up to the threshold (1:1 feel), then
 * asymptotic past it so the indicator still moves but never runs off
 * the screen. Pinning the curve so a stylistic tweak doesn't quietly
 * change the perceived "weight" of the gesture.
 */
import { describe, expect, it } from "vitest";
import {
  computePullDistance,
  computePullLayout,
} from "@/lib/use-pull-to-refresh";

const THRESHOLD = 80;
const MAX = 140;

describe("computePullDistance", () => {
  it("returns 0 for non-positive input", () => {
    expect(computePullDistance(0, THRESHOLD, MAX)).toBe(0);
    expect(computePullDistance(-50, THRESHOLD, MAX)).toBe(0);
  });

  it("is linear (1:1) up to the threshold", () => {
    expect(computePullDistance(10, THRESHOLD, MAX)).toBe(10);
    expect(computePullDistance(40, THRESHOLD, MAX)).toBe(40);
    expect(computePullDistance(THRESHOLD, THRESHOLD, MAX)).toBe(THRESHOLD);
  });

  it("asymptotically approaches max past the threshold", () => {
    const justOver = computePullDistance(THRESHOLD + 1, THRESHOLD, MAX);
    expect(justOver).toBeGreaterThan(THRESHOLD);
    expect(justOver).toBeLessThan(MAX);

    const farOver = computePullDistance(THRESHOLD + 1_000, THRESHOLD, MAX);
    expect(farOver).toBeLessThan(MAX);
    expect(farOver).toBeGreaterThan(THRESHOLD + (MAX - THRESHOLD) * 0.9);
  });

  it("never returns a value above the cap", () => {
    for (const raw of [200, 500, 1_000, 10_000]) {
      const d = computePullDistance(raw, THRESHOLD, MAX);
      expect(d).toBeLessThanOrEqual(MAX);
    }
  });

  it("adds room/2 when the overshoot equals the remaining room", () => {
    // Algebraic anchor for the curve: at overshoot = room, the curve
    // adds room/2. This is the perceptual midpoint of the indicator's
    // remaining travel.
    const room = MAX - THRESHOLD;
    const atOvershoot = computePullDistance(THRESHOLD + room, THRESHOLD, MAX);
    expect(atOvershoot).toBeCloseTo(THRESHOLD + room / 2, 5);
  });
});

describe("computePullLayout", () => {
  const H = 76;

  it("is fully at rest at distance 0", () => {
    const l = computePullLayout(0, THRESHOLD, H);
    expect(l).toEqual({
      contentOffset: 0,
      gap: 0,
      indicatorY: -H,
      progress: 0,
    });
  });

  it("moves the content by the whole pull distance, opening a matching gap", () => {
    for (const d of [10, 50, THRESHOLD, 120]) {
      const l = computePullLayout(d, THRESHOLD, H);
      expect(l.contentOffset).toBe(d);
      expect(l.gap).toBe(d);
    }
  });

  it("rides the gap's bottom edge while the gap is shorter than the indicator", () => {
    const l = computePullLayout(40, THRESHOLD, H);
    // Indicator bottom sits exactly on the gap bottom (the content's top).
    expect(l.indicatorY + H).toBe(40);
  });

  it("centres the indicator once the gap is taller, continuously", () => {
    const l = computePullLayout(120, THRESHOLD, H);
    expect(l.indicatorY).toBe((120 - H) / 2);
    expect(l.indicatorY + H).toBeLessThanOrEqual(120);
    // Both branches meet at distance === H.
    expect(computePullLayout(H, THRESHOLD, H).indicatorY).toBe(0);
    expect(computePullLayout(H - 0.001, THRESHOLD, H).indicatorY).toBeCloseTo(
      0,
      2,
    );
  });

  it("never lets the indicator overlap the content (bottom <= gap)", () => {
    for (let d = 0; d <= 140; d += 5) {
      const l = computePullLayout(d, THRESHOLD, H);
      expect(l.indicatorY + H).toBeLessThanOrEqual(l.gap + 1e-9);
    }
  });

  it("progress ramps to 1 at the threshold and stays there", () => {
    expect(computePullLayout(THRESHOLD / 2, THRESHOLD, H).progress).toBe(0.5);
    expect(computePullLayout(THRESHOLD, THRESHOLD, H).progress).toBe(1);
    expect(computePullLayout(THRESHOLD * 2, THRESHOLD, H).progress).toBe(1);
  });
});
