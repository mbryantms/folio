import { describe, expect, it } from "vitest";

import spec from "@/lib/api/openapi.json";
import { MARKER_PALETTE, isValidMarkerColor } from "@/lib/markers/palette";

type Schema = { properties: Record<string, { pattern?: string }> };
const schemas = (spec as { components: { schemas: Record<string, Schema> } })
  .components.schemas;

describe("marker colour palette (WP-5.3)", () => {
  it("matches the server's published pattern exactly", () => {
    const pattern = schemas.CreateMarkerReq!.properties.color!.pattern!;
    expect(pattern).toBe(
      `^(${MARKER_PALETTE.join("|")}|#[0-9A-Fa-f]{6}|#[0-9A-Fa-f]{8})$`,
    );
    expect(schemas.UpdateMarkerReq!.properties.color!.pattern).toBe(pattern);
  });

  it("accepts palette names and 6/8-digit hex, rejects the rest", () => {
    for (const ok of [...MARKER_PALETTE, "#a1B2c3", "#A1B2C3FF", ""]) {
      expect(isValidMarkerColor(ok)).toBe(true);
    }
    for (const bad of ["purple", "Yellow", "#abc", "#12345G", "red;"]) {
      expect(isValidMarkerColor(bad)).toBe(false);
    }
  });
});
