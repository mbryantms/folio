// @vitest-environment jsdom
/**
 * `StickyToolbarRow` — the `sm`-and-up pinned control row (phone-layout
 * rule 3). Pins under the topbar only while it measures one row and is
 * `enabled`; the chrome-band border / backdrop are gated on `stuck`,
 * false at rest. Extra `data-*` attributes reach the row element.
 */
import { act, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { StickyToolbarRow } from "@/components/library/StickyToolbarRow";
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

const row = () => document.querySelector<HTMLElement>("[data-toolbar-row]")!;

describe("StickyToolbarRow", () => {
  it("pins once measured at one row, with the band hidden at rest", () => {
    render(
      <StickyToolbarRow data-testrow="">
        <button type="button">Sort</button>
      </StickyToolbarRow>,
    );
    expect(row().hasAttribute("data-testrow")).toBe(true);
    expect(row().className).toContain("max-sm:hidden");
    expect(row().className).not.toMatch(/\bsticky\b/);
    act(() => fire?.([{ contentRect: { height: 36 } }]));
    expect(row().className).toMatch(/\bsticky\b/);
    expect(row().className).toMatch(/top-\(--topbar-h\)/);
    expect(row().hasAttribute("data-sticky")).toBe(true);
    expect(row().hasAttribute("data-stuck")).toBe(false);
    expect(row().className).not.toMatch(/\bborder-b\b/);
  });

  it("stays in flow when wrapped or disabled", () => {
    const { rerender } = render(
      <StickyToolbarRow>
        <button type="button">Sort</button>
      </StickyToolbarRow>,
    );
    act(() => fire?.([{ contentRect: { height: ONE_ROW_MAX_PX + 20 } }]));
    expect(row().className).not.toMatch(/\bsticky\b/);
    rerender(
      <StickyToolbarRow enabled={false}>
        <button type="button">Sort</button>
      </StickyToolbarRow>,
    );
    act(() => fire?.([{ contentRect: { height: 36 } }]));
    expect(row().className).not.toMatch(/\bsticky\b/);
  });

  it("runs edge to edge inside the shell column", () => {
    render(
      <StickyToolbarRow>
        <span />
      </StickyToolbarRow>,
    );
    const classes = row().className.split(/\s+/);
    expect(classes).toContain("-mx-4");
    expect(classes).toContain("md:-mx-8");
  });
});
