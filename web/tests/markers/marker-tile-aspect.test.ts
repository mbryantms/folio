/**
 * `markerTileAspect` — the saved-markers grid tile aspect (archive bug fix).
 *
 * A highlight region renders distorted when the tile aspect assumes a 2:3
 * page but the page is actually a different shape (e.g. a double-page
 * spread). The capture-time `page_w`/`page_h` stamp lets the tile use the
 * page's true aspect; markers without the stamp fall back to 2:3.
 */
import { describe, expect, it } from "vitest";
import { markerTileAspect } from "@/components/markers/MarkersList";
import type { MarkerView } from "@/lib/api/types";

const PAGE_ASPECT = 2 / 3;

function marker(region: MarkerView["region"]): MarkerView {
  return {
    id: "m1",
    issue_id: "i1",
    page_index: 0,
    kind: "highlight",
    region,
  } as MarkerView;
}

describe("markerTileAspect", () => {
  it("falls back to 2:3 for page-level (no region) markers", () => {
    expect(markerTileAspect(marker(null))).toBeCloseTo(PAGE_ASPECT);
  });

  it("uses the 2:3 approximation when the region carries no page dims", () => {
    // A square region on an assumed-2:3 page → (1)*（2/3).
    const a = markerTileAspect(
      marker({ x: 0, y: 0, w: 50, h: 50, shape: "rect" }),
    );
    expect(a).toBeCloseTo(PAGE_ASPECT);
  });

  it("uses the real page aspect when page_w/page_h are stamped", () => {
    // Double-page spread: 4000×2500 ≈ 1.6 landscape. A region covering the
    // full width and half height → (w/h)=(100/50)=2, ×1.6 = 3.2 — landscape,
    // NOT the squished ~1.33 the 2:3 assumption would give.
    const a = markerTileAspect(
      marker({
        x: 0,
        y: 0,
        w: 100,
        h: 50,
        shape: "rect",
        page_w: 4000,
        page_h: 2500,
      }),
    );
    expect(a).toBeCloseTo(2 * (4000 / 2500));
  });

  it("ignores degenerate page dims (height 0) and falls back", () => {
    const a = markerTileAspect(
      marker({
        x: 0,
        y: 0,
        w: 50,
        h: 50,
        shape: "rect",
        page_w: 4000,
        page_h: 0,
      }),
    );
    expect(a).toBeCloseTo(PAGE_ASPECT);
  });
});
