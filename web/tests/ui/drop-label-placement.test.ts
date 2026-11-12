import { readFileSync } from "node:fs";
import { join } from "node:path";

import { describe, expect, it } from "vitest";

import { dropLabelPlacement } from "@/lib/ui/springy-drag";

describe("dropLabelPlacement", () => {
  const row = { width: 1100, height: 48 };

  it("clears the tilted row: more room left of centre, a gap everywhere", () => {
    const farLeft = dropLabelPlacement({ ...row, offset: 20 }, false);
    const centre = dropLabelPlacement({ ...row, offset: 562 }, false);
    const right = dropLabelPlacement({ ...row, offset: 1000 }, false);
    // At the left end the row hangs ~12 px below its box.
    expect(farLeft.top).toBeGreaterThanOrEqual(6 + 12);
    expect(centre.top).toBeLessThan(farLeft.top);
    // Right of centre the row lifts away; only the scale and the gap.
    expect(right.top).toBe(Math.ceil(48 * 0.02) + 6);
    expect(farLeft.left).toBe(8);
    expect(right.left).toBe(988);
  });

  it("with reduced motion (no tilt) keeps just the scale and the gap", () => {
    expect(dropLabelPlacement({ ...row, offset: 20 }, true).top).toBe(
      Math.ceil(48 * 0.02) + 6,
    );
  });

  it("mirrors the .drag-lift rule it compensates for", () => {
    const css = readFileSync(
      join(__dirname, "../../styles/globals.css"),
      "utf8",
    );
    const rule = css.match(/\.drag-lift \{([^}]*)\}/)?.[1] ?? "";
    expect(rule).toMatch(/scale: 1\.04;/);
    expect(rule).toMatch(/rotate: -1\.25deg;/);
  });
});
