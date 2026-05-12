import { describe, expect, it } from "vitest";

import { validateCron } from "@/lib/api/cron";

describe("validateCron", () => {
  it("treats empty / whitespace as a no-schedule, ok-state", () => {
    const r = validateCron("");
    expect(r.ok).toBe(true);
    if (r.ok) {
      expect(r.humanized).toMatch(/never/i);
      expect(r.nextRuns).toHaveLength(0);
    }
    const r2 = validateCron("   ");
    expect(r2.ok).toBe(true);
  });

  it("humanizes a hourly cron and produces 3 future runs", () => {
    const r = validateCron("0 * * * *");
    expect(r.ok).toBe(true);
    if (r.ok) {
      expect(r.humanized.toLowerCase()).toContain("hour");
      expect(r.nextRuns).toHaveLength(3);
      // All future-dated.
      for (const d of r.nextRuns) {
        expect(d.getTime()).toBeGreaterThan(Date.now() - 1000);
      }
    }
  });

  it("rejects an invalid cron expression", () => {
    const r = validateCron("not-a-cron");
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.error).toBeTruthy();
  });

  it("accepts a 6-hour interval and humanizes", () => {
    const r = validateCron("0 */6 * * *");
    expect(r.ok).toBe(true);
    if (r.ok) {
      expect(r.humanized.toLowerCase()).toMatch(/(every|hour)/);
      expect(r.nextRuns).toHaveLength(3);
    }
  });
});
