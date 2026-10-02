// @vitest-environment jsdom
import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { MarkerView } from "@/lib/api/types";
import {
  KEYBIND_DEFAULTS,
  KEYBIND_LABELS,
  READER_KEYBIND_ACTIONS,
  findConflict,
} from "@/lib/reader/keybinds";
import {
  nextDrawerIndex,
  sortDrawerMarkers,
  useMarkerDrawer,
} from "@/lib/reader/marker-drawer";

function marker(overrides: Partial<MarkerView>): MarkerView {
  return {
    id: "m",
    user_id: "u",
    series_id: "s",
    issue_id: "i1",
    page_index: 0,
    kind: "bookmark",
    is_favorite: false,
    tags: [],
    region: null,
    selection: null,
    body: null,
    color: null,
    hidden_from_log: false,
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
    ...overrides,
  };
}

const ITEMS: MarkerView[] = [
  marker({ id: "c", page_index: 7, kind: "note", body: "later note" }),
  marker({
    id: "a",
    page_index: 2,
    kind: "highlight",
    selection: { text: "captured words" },
  }),
  marker({ id: "b", page_index: 2, created_at: "2026-01-02T00:00:00Z" }),
];

vi.mock("@/lib/api/queries", () => ({
  useIssueMarkers: () => ({
    data: { items: ITEMS },
    isLoading: false,
    isError: false,
  }),
}));

// Imported after the mock is registered.
const { MarkerDrawer } =
  await import("@/app/[locale]/read/[seriesSlug]/[issueSlug]/MarkerDrawer");

describe("marker drawer helpers", () => {
  it("sorts by page, then creation time", () => {
    expect(sortDrawerMarkers(ITEMS).map((m) => m.id)).toEqual(["a", "b", "c"]);
  });

  it("wraps arrow navigation and maps Home / End", () => {
    expect(nextDrawerIndex("ArrowDown", 2, 3)).toBe(0);
    expect(nextDrawerIndex("ArrowUp", 0, 3)).toBe(2);
    expect(nextDrawerIndex("Home", 2, 3)).toBe(0);
    expect(nextDrawerIndex("End", 0, 3)).toBe(2);
    expect(nextDrawerIndex("Enter", 1, 3)).toBeNull();
    expect(nextDrawerIndex("ArrowDown", 0, 0)).toBeNull();
  });

  it("binds `l` without conflicts", () => {
    expect(READER_KEYBIND_ACTIONS).toContain("toggleMarkerDrawer");
    expect(KEYBIND_DEFAULTS.toggleMarkerDrawer).toBe("l");
    expect(KEYBIND_LABELS.toggleMarkerDrawer).toMatch(/markers/i);
    expect(
      findConflict("l", "toggleMarkerDrawer", { ...KEYBIND_DEFAULTS }),
    ).toBeNull();
  });
});

describe("MarkerDrawer", () => {
  beforeEach(() => {
    act(() => useMarkerDrawer.getState().setOpen(true));
  });
  afterEach(() => {
    act(() => useMarkerDrawer.getState().setOpen(false));
  });

  it("lists the issue's markers in page order and marks the current page", () => {
    render(<MarkerDrawer issueId="i1" currentPage={2} onJump={() => {}} />);
    const buttons = screen.getAllByRole("button", { name: /^Page/ });
    expect(buttons.map((b) => b.textContent)).toEqual([
      expect.stringContaining("Page 3 · Highlight"),
      expect.stringContaining("Page 3 · Bookmark"),
      expect.stringContaining("Page 8 · Note"),
    ]);
    expect(buttons[0]!.textContent).toContain("captured words");
    expect(buttons[0]!.getAttribute("aria-current")).toBe("page");
    expect(buttons[2]!.hasAttribute("aria-current")).toBe(false);
  });

  it("is keyboard navigable with roving focus and keeps keys from the reader", () => {
    const onJump = vi.fn();
    const readerKeys = vi.fn();
    window.addEventListener("keydown", readerKeys);
    try {
      render(<MarkerDrawer issueId="i1" currentPage={0} onJump={onJump} />);
      const buttons = screen.getAllByRole("button", { name: /^Page/ });
      // Single tab stop.
      expect(buttons.map((b) => b.tabIndex)).toEqual([0, -1, -1]);

      buttons[0]!.focus();
      fireEvent.keyDown(buttons[0]!, { key: "ArrowDown" });
      expect(document.activeElement).toBe(buttons[1]);
      fireEvent.keyDown(buttons[1]!, { key: "End" });
      expect(document.activeElement).toBe(buttons[2]);
      fireEvent.keyDown(buttons[2]!, { key: "ArrowDown" });
      expect(document.activeElement).toBe(buttons[0]);
      fireEvent.keyDown(buttons[0]!, { key: "ArrowUp" });
      expect(document.activeElement).toBe(buttons[2]);
      expect(buttons[2]!.tabIndex).toBe(0);

      // None of the list's own keys reached the reader keymap.
      expect(readerKeys).not.toHaveBeenCalled();

      fireEvent.click(buttons[2]!);
      expect(onJump).toHaveBeenCalledWith(7);
    } finally {
      window.removeEventListener("keydown", readerKeys);
    }
  });
});
