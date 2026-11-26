/**
 * The catalog browser groups the files of one reading order (CMRO's
 * 1,000-issue parts, a Marvel Guides era) so they can be imported
 * together, and labels each file's source.
 */
import { describe, expect, it } from "vitest";

import { catalogOrders, sourceLabel } from "@/components/cbl/cbl-import-dialog";
import type { CatalogEntryView } from "@/lib/api/types";

function file(
  name: string,
  over: Partial<CatalogEntryView> = {},
): CatalogEntryView {
  return {
    path: `folder/${name}.cbl`,
    name,
    publisher: "Marvel",
    sha: "0".repeat(40),
    size: 100,
    source: "cmro",
    ...over,
  } as CatalogEntryView;
}

describe("catalogOrders", () => {
  it("groups parts that share an order, in part order", () => {
    const core = { order_key: "k|core", order_name: "CMRO Core" };
    const orders = catalogOrders([
      file("Core 10", { ...core, part: 10 }),
      file("Core 2", { ...core, part: 2 }),
      file("Standalone"),
      file("Core 1", { ...core, part: 1 }),
      file("Guide 1.4", {
        order_key: "k|mg",
        order_name: "2012-2015",
        part: 1.4,
      }),
      file("Guide 2.1", {
        order_key: "k|mg",
        order_name: "2012-2015",
        part: 2.1,
      }),
    ]);
    expect(orders.map((o) => o.name)).toEqual(["2012-2015", "CMRO Core"]);
    expect(orders[1]!.parts.map((p) => p.name)).toEqual([
      "Core 1",
      "Core 2",
      "Core 10",
    ]);
    expect(orders[0]!.parts.map((p) => p.part)).toEqual([1.4, 2.1]);
  });

  it("ignores an order with a single file here", () => {
    expect(
      catalogOrders([
        file("Only", { order_key: "k", order_name: "X", part: 1 }),
      ]),
    ).toEqual([]);
  });
});

describe("sourceLabel", () => {
  it("names the source, with the CMRO depth", () => {
    expect(sourceLabel("cmro", "Core")?.label).toBe("CMRO · Core");
    expect(sourceLabel("cbro_with_events")?.label).toBe("CBRO · with events");
    expect(sourceLabel("marvel_guides")?.label).toBe("Marvel Guides");
    expect(sourceLabel("other")).toBeNull();
  });
});
