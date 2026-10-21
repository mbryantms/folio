// @vitest-environment jsdom
/**
 * Desktop library toolbar stickiness (`useStickyToolbar`). The row pins
 * under the topbar only while it fits on one line and select mode is
 * off; a wrapped toolbar stays in flow, and in select mode the sticky
 * `SelectionToolbar` owns the band instead. The border / backdrop
 * classes are gated on `stuck`, which is false at rest.
 */
import { act, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { LibraryGridToolbar } from "@/components/library/LibraryGridToolbar";
import { ONE_ROW_MAX_PX } from "@/lib/ui/use-sticky-toolbar";

type RoCallback = (entries: Array<{ contentRect: { height: number } }>) => void;
let fire: RoCallback | null = null;

beforeEach(() => {
  fire = null;
  vi.stubGlobal(
    "ResizeObserver",
    class {
      constructor(cb: RoCallback) {
        fire = cb;
      }
      observe() {}
      unobserve() {}
      disconnect() {}
    },
  );
});
afterEach(() => vi.unstubAllGlobals());

function renderToolbar(selectMode = false) {
  const noop = () => {};
  return render(
    <LibraryGridToolbar
      mode="series"
      onMode={noop}
      q=""
      onQ={noop}
      trimmedQ=""
      seriesSort="name"
      onSeriesSort={noop}
      issueSort="number"
      onIssueSort={noop}
      order="asc"
      onOrder={noop}
      facetCount={0}
      onOpenFilters={noop}
      canSaveView={false}
      onSaveView={noop}
      onClearFacets={noop}
      cardSize={160}
      onCardSize={noop}
      cardSizeMin={96}
      cardSizeMax={320}
      cardSizeStep={8}
      cardSizeDefault={160}
      canSelect
      selectMode={selectMode}
      onEnterSelect={noop}
      onExitSelect={noop}
    />,
  );
}

const toolbar = () =>
  document.querySelector<HTMLElement>("[data-library-toolbar]")!;

describe("LibraryGridToolbar stickiness", () => {
  it("sticks once measured at one row, and not at rest", () => {
    renderToolbar();
    expect(toolbar().className).not.toMatch(/\bsticky\b/);
    act(() => fire?.([{ contentRect: { height: 36 } }]));
    expect(toolbar().className).toMatch(/\bsticky\b/);
    expect(toolbar().className).toMatch(/top-\(--topbar-h\)/);
    // Sentinel and toolbar are adjacent (both 0×0 in jsdom) → not stuck,
    // so the band's border / backdrop are absent at rest.
    expect(toolbar().hasAttribute("data-stuck")).toBe(false);
    expect(toolbar().className).not.toMatch(/\bborder-b\b/);
  });

  it("stays in flow when the row has wrapped", () => {
    renderToolbar();
    act(() => fire?.([{ contentRect: { height: ONE_ROW_MAX_PX + 20 } }]));
    expect(toolbar().className).not.toMatch(/\bsticky\b/);
  });

  it("yields the band to the selection toolbar in select mode", () => {
    renderToolbar(true);
    act(() => fire?.([{ contentRect: { height: 36 } }]));
    expect(toolbar().className).not.toMatch(/\bsticky\b/);
  });

  it("runs edge to edge inside the shell column", () => {
    renderToolbar();
    const classes = toolbar().className.split(/\s+/);
    expect(classes).toContain("-mx-4");
    expect(classes).toContain("md:-mx-8");
  });
});
