/**
 * Shaped loading stand-ins (`components/skeletons/LoadingShapes`): every
 * shape is an aria-busy region with hidden boxes and one spoken label, and
 * cover shapes carry the shared cover silhouette.
 */
import { describe, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { createElement } from "react";

vi.mock("@/lib/visual-effects");

import {
  CoverGridSkeleton,
  EntityDetailSkeleton,
  LoadingRegion,
  PageRailsSkeleton,
  PublisherDetailSkeleton,
  RailsSkeleton,
  RowsSkeleton,
  TableRowsSkeleton,
  TilesSkeleton,
} from "@/components/skeletons/LoadingShapes";
import { COVER_GRID_COLUMNS, COVER_WIDTH } from "@/lib/library/cover-size";

/** The app-wide cover grid / rail-card width, as serialized inline. */
const COVER_GRID_CSS = `grid-template-columns:${COVER_GRID_COLUMNS}`;
const COVER_WIDTH_CSS = `width:${COVER_WIDTH}`;

const count = (html: string, needle: string) => html.split(needle).length - 1;

describe("loading shapes", () => {
  it("cover grid renders cover silhouettes under one label", () => {
    const html = renderToStaticMarkup(
      createElement(CoverGridSkeleton, { count: 6, label: "Loading entries…" }),
    );
    expect(html).toContain('aria-busy="true"');
    expect(count(html, "cover-skeleton__masthead")).toBe(6);
    expect(count(html, 'role="status"')).toBe(1);
    expect(html).toContain("Loading entries…");
  });

  it("cover grid uses the app-wide cover template with no size prop", () => {
    // The columns come from `--cover-min` on <html>, which the pre-paint
    // bootstrap sets — so the skeleton already has the reader's column
    // count, and no surface passes a size or a storage key.
    const html = renderToStaticMarkup(createElement(CoverGridSkeleton));
    expect(count(html, COVER_GRID_CSS)).toBe(1);
    expect(html).toContain("var(--cover-min, 160px)");
  });

  it("uses the caller's grid template for a grid that is not the cover grid", () => {
    const html = renderToStaticMarkup(
      createElement(CoverGridSkeleton, {
        style: { gridTemplateColumns: "repeat(auto-fill, minmax(123px, 1fr))" },
      }),
    );
    expect(html).toContain("minmax(123px, 1fr)");
    expect(html).not.toContain(COVER_GRID_CSS);
  });

  it("rails render one header per rail", () => {
    const html = renderToStaticMarkup(
      createElement(RailsSkeleton, { rails: 3, label: "Loading views…" }),
    );
    // 16 cards per rail: enough to cross a wide screen at the smallest
    // cover size.
    expect(count(html, "cover-skeleton__masthead")).toBe(48);
    expect(html).toContain("Loading views…");
  });

  it("rail cards take the library grid's cover width", () => {
    const html = renderToStaticMarkup(
      createElement(RailsSkeleton, { rails: 2, card: "series" }),
    );
    // Every card plus the "View all" tail, per rail — the same
    // `--cover-w` width the loaded rail's cards use, so nothing resizes
    // when the content lands.
    expect(count(html, COVER_WIDTH_CSS)).toBe(2 * (16 + 1));
    expect(html).not.toMatch(/width:\d+px/);
  });

  it("the rails page skeleton nests the same cover-width rails under one label", () => {
    const html = renderToStaticMarkup(
      createElement(PageRailsSkeleton, { rails: 3, label: "Loading home…" }),
    );
    expect(count(html, COVER_WIDTH_CSS)).toBe(3 * (16 + 1));
    expect(count(html, 'role="status"')).toBe(1);
    expect(html).toContain("Loading home…");
  });

  it("an empty label renders no status node (nested shapes)", () => {
    const html = renderToStaticMarkup(
      createElement(RowsSkeleton, { rows: 3, label: "" }),
    );
    expect(html).not.toContain('role="status"');
    expect(count(html, "<li")).toBe(3);
  });

  it("table rows follow the table's column template", () => {
    const html = renderToStaticMarkup(
      createElement(TableRowsSkeleton, {
        rows: 2,
        columns: 4,
        gridClassName: "grid-cols-[1fr_2fr_1fr_1fr]",
      }),
    );
    expect(count(html, "grid-cols-[1fr_2fr_1fr_1fr]")).toBe(2);
  });

  it("tiles keep a phone-width grid-cols-1 base by default", () => {
    const html = renderToStaticMarkup(createElement(TilesSkeleton));
    expect(html).toContain("grid-cols-1");
  });

  it("entity detail has a header, the profile card and the insights", () => {
    const html = renderToStaticMarkup(createElement(EntityDetailSkeleton));
    expect(html).toContain("border-b pb-4");
    // Profile card: the 176x256 image beside the fact grid.
    expect(html).toContain("h-64 w-44");
    expect(html).toContain("lg:grid-cols-3");
    // Four stat tiles, then the earliest-appearances rail of 120px cards.
    expect(html).toContain("grid-cols-2 gap-3 lg:grid-cols-4");
    expect(count(html, "w-[120px]")).toBe(10);
  });

  it("region layout classes space the boxes themselves", () => {
    const html = renderToStaticMarkup(
      <LoadingRegion label="Loading…" className="space-y-6">
        <i />
        <b />
      </LoadingRegion>,
    );
    // The class sits on the element whose direct children are the boxes,
    // not on a wrapper around a `display: contents` div.
    expect(html).toContain(
      '<div aria-hidden="true" class="space-y-6"><i></i><b></b></div>',
    );
    expect(html).not.toContain("contents");
  });
});

describe("route skeletons", () => {
  it("entity indexes end on the component's own grid skeleton", async () => {
    const { EntityIndexSkeleton } =
      await import("@/components/skeletons/EntityIndexSkeleton");
    const html = renderToStaticMarkup(
      createElement(EntityIndexSkeleton, { label: "Loading teams…" }),
    );
    expect(html).toContain("Loading teams…");
    expect(html).toContain("minmax(220px, 1fr)");
    expect(html).not.toContain("cover-skeleton");
  });

  it("publishers' detail skeleton is a header over a series grid", () => {
    const html = renderToStaticMarkup(createElement(PublisherDetailSkeleton));
    expect(html).not.toContain("h-64 w-44");
    // The series grid is the app-wide cover grid, not a fixed 160px one.
    expect(count(html, COVER_GRID_CSS)).toBe(1);
    expect(html).not.toContain("minmax(160px, 1fr)");
    expect(count(html, "cover-skeleton__masthead")).toBe(12);
  });

  it("entity index rail spreads # + A-Z like the real jump rail", async () => {
    const { EntityIndexSkeleton } =
      await import("@/components/skeletons/EntityIndexSkeleton");
    const html = renderToStaticMarkup(
      createElement(EntityIndexSkeleton, { label: "Loading arcs…" }),
    );
    expect(html).toContain("md:justify-between");
    expect(count(html, "grid size-6 place-items-center")).toBe(27);
  });

  it("year in review draws the story frame, not overview tiles", async () => {
    const { default: YearLoading } =
      await import("@/app/[locale]/(library)/log/year/[year]/loading");
    const html = renderToStaticMarkup(createElement(YearLoading));
    expect(html).toContain("fixed inset-0 z-50");
    expect(html).toContain("rounded-3xl");
    expect(html).toContain("Loading your year in review…");
  });

  it("downloads renders static rows with no cover-card modules", async () => {
    const { default: DownloadsLoading } =
      await import("@/app/[locale]/downloads/loading");
    const html = renderToStaticMarkup(createElement(DownloadsLoading));
    expect(html).toContain("Loading downloads…");
    expect(html.split("<li").length - 1).toBe(5);
  });
});
