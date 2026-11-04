import { describe, expect, it } from "vitest";

import { decodeTone, encodeTone, toneKey } from "@/lib/ui/cover-tone";

describe("cover tone memory", () => {
  it("round-trips a palette through the compact encoding", () => {
    const v = encodeTone({ average: [16, 32, 255], vibrant: [200, 0, 7] });
    expect(v).toBe("1020ffc80007");
    expect(decodeTone(v)).toEqual({
      average: "16 32 255",
      vibrant: "200 0 7",
    });
  });

  it("rejects junk entries", () => {
    expect(decodeTone(undefined)).toBeNull();
    expect(decodeTone("zzzzzz000000")).toBeNull();
    expect(decodeTone("123")).toBeNull();
  });

  it("keys covers without their size query", () => {
    expect(toneKey("/issues/a/pages/0/thumb?w=300")).toBe(
      "/issues/a/pages/0/thumb",
    );
    expect(toneKey("/x.jpg")).toBe("/x.jpg");
  });
});
