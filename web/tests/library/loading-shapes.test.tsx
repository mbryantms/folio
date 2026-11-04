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
  RailsSkeleton,
  RowsSkeleton,
  TableRowsSkeleton,
  TilesSkeleton,
} from "@/components/skeletons/LoadingShapes";

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

  it("uses the caller's grid template so columns match the content", () => {
    const html = renderToStaticMarkup(
      createElement(CoverGridSkeleton, {
        style: { gridTemplateColumns: "repeat(auto-fill, minmax(123px, 1fr))" },
      }),
    );
    expect(html).toContain("minmax(123px, 1fr)");
  });

  it("rails render one header per rail", () => {
    const html = renderToStaticMarkup(
      createElement(RailsSkeleton, { rails: 3, label: "Loading views…" }),
    );
    expect(count(html, "cover-skeleton__masthead")).toBe(24);
    expect(html).toContain("Loading views…");
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

  it("entity detail has a header, tabs and a cover grid", () => {
    const html = renderToStaticMarkup(createElement(EntityDetailSkeleton));
    expect(html).toContain("border-b pb-4");
    expect(count(html, "cover-skeleton__masthead")).toBe(12);
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

  it("publishers' detail skeleton has no tab strip", () => {
    const withTabs = renderToStaticMarkup(createElement(EntityDetailSkeleton));
    const without = renderToStaticMarkup(
      createElement(EntityDetailSkeleton, { tabs: false }),
    );
    expect(withTabs).toContain("bg-muted/50");
    expect(without).not.toContain("bg-muted/50");
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
