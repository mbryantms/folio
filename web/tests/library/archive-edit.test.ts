import { describe, expect, it } from "vitest";

import {
  buildOps,
  hasChanges,
  initialSlots,
  rotateBy,
  summarizeOps,
  type PageSlot,
} from "@/lib/archive-edit";

const pristine = (n: number) => initialSlots(n);

describe("buildOps", () => {
  it("emits nothing for a pristine list", () => {
    expect(buildOps(pristine(4))).toEqual([]);
    expect(hasChanges(pristine(4))).toBe(false);
  });

  it("emits Remove ops highest-index-first", () => {
    const slots = pristine(4);
    slots[1]!.removed = true;
    slots[3]!.removed = true;
    expect(buildOps(slots)).toEqual([
      { kind: "remove", ordinal: 3 },
      { kind: "remove", ordinal: 1 },
    ]);
  });

  it("emits a Reorder permutation over survivors", () => {
    // Drag p3 (orig 2) to the front: display order [2,0,1].
    const slots: PageSlot[] = [
      {
        orig: 2,
        rotation: 0,
        removed: false,
        replaceId: null,
        transform: null,
      },
      {
        orig: 0,
        rotation: 0,
        removed: false,
        replaceId: null,
        transform: null,
      },
      {
        orig: 1,
        rotation: 0,
        removed: false,
        replaceId: null,
        transform: null,
      },
    ];
    expect(buildOps(slots)).toEqual([
      { kind: "reorder", new_order: [2, 0, 1] },
    ]);
  });

  it("emits Rotate with the degrees enum at the final position", () => {
    const slots = pristine(2);
    slots[1]!.rotation = 90;
    expect(buildOps(slots)).toEqual([
      { kind: "rotate", ordinal: 1, degrees: "r90" },
    ]);
  });

  it("emits Replace at the final position", () => {
    const slots = pristine(2);
    slots[0]!.replaceId = "abc";
    expect(buildOps(slots)).toEqual([
      { kind: "replace", ordinal: 0, image_id: "abc" },
    ]);
  });

  it("orders removes → reorder → rotate → replace for a combined edit", () => {
    // Original 4 pages. Remove orig 1; reorder survivors to [3,0,2];
    // rotate the page now at display 0 (orig 3) by 180; replace display 2.
    const slots: PageSlot[] = [
      {
        orig: 3,
        rotation: 180,
        removed: false,
        replaceId: null,
        transform: null,
      },
      {
        orig: 0,
        rotation: 0,
        removed: false,
        replaceId: null,
        transform: null,
      },
      {
        orig: 2,
        rotation: 0,
        removed: false,
        replaceId: "img",
        transform: null,
      },
      { orig: 1, rotation: 0, removed: true, replaceId: null, transform: null },
    ];
    expect(buildOps(slots)).toEqual([
      { kind: "remove", ordinal: 1 },
      { kind: "reorder", new_order: [2, 0, 1] },
      { kind: "rotate", ordinal: 0, degrees: "r180" },
      { kind: "replace", ordinal: 2, image_id: "img" },
    ]);
  });

  it("emits a Transform op at the final position", () => {
    const slots = pristine(2);
    slots[1]!.transform = [
      { kind: "brightness_contrast", brightness: 10, contrast: 0 },
      { kind: "crop_box", x: 0, y: 0, w: 50, h: 80 },
    ];
    expect(buildOps(slots)).toEqual([
      {
        kind: "transform",
        ordinal: 1,
        chain: [
          { kind: "brightness_contrast", brightness: 10, contrast: 0 },
          { kind: "crop_box", x: 0, y: 0, w: 50, h: 80 },
        ],
      },
    ]);
  });

  it("ignores an empty transform chain", () => {
    const slots = pristine(1);
    slots[0]!.transform = [];
    expect(buildOps(slots)).toEqual([]);
  });

  it("treats a net-zero rotation as no change", () => {
    const slots = pristine(1);
    slots[0]!.rotation = rotateBy(rotateBy(slots[0]!.rotation, 90), 270); // 360 → 0
    expect(slots[0]!.rotation).toBe(0);
    expect(buildOps(slots)).toEqual([]);
  });
});

describe("rotateBy", () => {
  it("wraps at 360", () => {
    expect(rotateBy(270, 90)).toBe(0);
    expect(rotateBy(0, 90)).toBe(90);
    expect(rotateBy(180, 180)).toBe(0);
  });
});

describe("summarizeOps", () => {
  it("renders human-readable lines", () => {
    const lines = summarizeOps([
      { kind: "remove", ordinal: 4 },
      { kind: "reorder", new_order: [2, 0, 1] },
      { kind: "rotate", ordinal: 2, degrees: "r90" },
      { kind: "replace", ordinal: 0, image_id: "x" },
      {
        kind: "transform",
        ordinal: 1,
        chain: [
          { kind: "brightness_contrast", brightness: 5, contrast: 0 },
          { kind: "crop_box", x: 0, y: 0, w: 10, h: 10 },
        ],
      },
    ]);
    expect(lines).toEqual([
      "Remove page 5",
      "Reorder pages (3 pages)",
      "Rotate page 3 by 90°",
      "Replace page 1",
      "Adjust page 2 — brightness/contrast, crop",
    ]);
  });
});
