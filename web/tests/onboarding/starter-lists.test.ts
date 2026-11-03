import { describe, expect, it } from "vitest";

import { groupStarterLists } from "@/components/onboarding/StarterListsPanel";
import { TOUR_STEPS } from "@/components/onboarding/tour-steps";
import type { StarterListView } from "@/lib/api/starter-lists";

const item = (
  title: string,
  publisher: string,
  available: number,
): StarterListView => ({
  catalog_source_id: "00000000-0000-0000-0000-000000001001",
  catalog_path: `${publisher}/${title}.cbl`,
  title,
  publisher,
  reason: "",
  origin: "shipped",
  hidden: false,
  override_id: null,
  coverage_status: "ready",
  total: 10,
  available,
  coverage_computed_at: null,
  imported_view_id: null,
});

describe("groupStarterLists", () => {
  it("leads with the publisher this library holds most of", () => {
    const groups = groupStarterLists([
      item("All-Star Superman", "DC", 0),
      item("Hawkeye", "Marvel", 10),
      item("Invincible", "Image", 4),
    ]);
    expect(groups.map((g) => g.publisher)).toEqual(["Marvel", "Image", "DC"]);
  });

  it("keeps curated order within a publisher, readable lists first", () => {
    const groups = groupStarterLists([
      item("A", "Marvel", 0),
      item("B", "Marvel", 3),
      item("C", "Marvel", 1),
    ]);
    expect(groups[0]!.lists.map((l) => l.title)).toEqual(["B", "C", "A"]);
  });
});

describe("TOUR_STEPS", () => {
  it("has unique ids and ends on the starter shelf", () => {
    const ids = TOUR_STEPS.map((s) => s.id);
    expect(new Set(ids).size).toBe(ids.length);
    expect(TOUR_STEPS.at(-1)!.kind).toBe("starter-lists");
    expect(TOUR_STEPS[0]!.anchors).toBeUndefined();
  });
});
