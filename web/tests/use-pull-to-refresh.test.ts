/**
 * Pure resistance-curve math for `usePullToRefresh`. The shape of
 * the curve matters: linear up to the threshold (1:1 feel), then
 * asymptotic past it so the indicator still moves but never runs off
 * the screen. Pinning the curve so a stylistic tweak doesn't quietly
 * change the perceived "weight" of the gesture.
 */
import { describe, expect, it } from "vitest";
import { computePullDistance } from "@/lib/use-pull-to-refresh";

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
