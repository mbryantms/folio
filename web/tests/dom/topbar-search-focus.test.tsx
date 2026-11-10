// @vitest-environment jsdom
/**
 * Header search popover focus handling. The panel opens on focus and
 * used to close only on an outside pointerdown or Escape, so tabbing
 * out left it open over the sidebar while focus walked its rows. It now
 * closes when focus leaves the search root; the rows are driven with
 * ↑/↓ from the input and sit out of the Tab order, while the panel's
 * other buttons stay tabbable.
 */
import { act, fireEvent, render, screen } from "@testing-library/react";
import { Sun, SunMoon } from "lucide-react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { SEARCH_CATEGORIES } from "@/lib/search/types";

const push = vi.fn();
vi.mock("next/navigation", () => ({
  useRouter: () => ({ push }),
  usePathname: () => "/",
}));

const emptyGroups = Object.fromEntries(
  SEARCH_CATEGORIES.map((def) => [def.key, []]),
);
vi.mock("@/lib/search/use-search", () => ({
  useGlobalSearch: () => ({
    enabled: false,
    isLoading: false,
    groups: emptyGroups,
    total: 0,
  }),
}));

const run = vi.fn();
const ACTIONS = [
  {
    id: "cmd-toggle-theme",
    label: "Toggle theme",
    group: "Commands",
    icon: Sun,
  },
  {
    id: "nav-theme-density",
    label: "Theme & density",
    group: "Settings",
    icon: SunMoon,
    href: "/settings/theme",
  },
];
vi.mock("@/lib/search/use-search-actions", () => ({
  useSearchActions: () => ({
    actions: ACTIONS,
    run,
    detailFor: (a: { group: string }) => a.group,
    keyHintFor: () => null,
  }),
}));

const { TopbarSearchInline } = await import("@/components/TopbarSearchInline");

function setup() {
  render(
    <>
      <TopbarSearchInline />
      <a href="/library">Next control</a>
    </>,
  );
  const input = screen.getByRole("searchbox", { name: "Search the library" });
  const outside = screen.getByRole("link", { name: "Next control" });
  return { input, outside };
}

const panel = () => screen.queryByRole("listbox", { name: "Search results" });

describe("TopbarSearchInline focus handling", () => {
  beforeEach(() => {
    push.mockReset();
    run.mockReset();
    window.localStorage.clear();
  });

  it("focusing the input opens the popover", () => {
    const { input } = setup();
    expect(panel()).toBeNull();
    act(() => input.focus());
    expect(panel()).not.toBeNull();
  });

  it("closes when focus moves to an element outside the root", () => {
    const { input, outside } = setup();
    act(() => input.focus());
    expect(panel()).not.toBeNull();
    act(() => outside.focus());
    expect(panel()).toBeNull();
  });

  it("closes when focus leaves the window", () => {
    const { input } = setup();
    act(() => input.focus());
    act(() => input.blur());
    expect(panel()).toBeNull();
  });

  it("keeps its rows out of the Tab order and points the input at the highlighted row", () => {
    const { input } = setup();
    act(() => input.focus());
    const options = screen.getAllByRole("option");
    expect(options.length).toBe(ACTIONS.length);
    for (const opt of options) expect(opt.getAttribute("tabindex")).toBe("-1");
    expect(input.getAttribute("aria-activedescendant")).toBe(options[0]!.id);
    fireEvent.keyDown(input, { key: "ArrowDown" });
    expect(input.getAttribute("aria-activedescendant")).toBe(options[1]!.id);
  });

  it("leaves the panel's other buttons in the Tab order and stays open while one has focus", () => {
    // The chips and "Clear all" have no arrow-key path, so Tab is the
    // only keyboard route to them.
    window.localStorage.setItem(
      "folio.search.recents.v1",
      JSON.stringify(["batman"]),
    );
    const { input, outside } = setup();
    act(() => input.focus());
    const clear = screen.getByRole("button", { name: "Clear all" });
    const chip = screen.getByRole("button", {
      name: "Search again for batman",
    });
    expect(clear.getAttribute("tabindex")).toBeNull();
    expect(chip.getAttribute("tabindex")).toBeNull();
    act(() => clear.focus());
    expect(panel()).not.toBeNull();
    act(() => outside.focus());
    expect(panel()).toBeNull();
  });

  it("a click on a row still activates it", () => {
    const { input } = setup();
    act(() => input.focus());
    const row = screen.getByRole("option", { name: /Toggle theme/ });
    // Browsers that don't focus a clicked button blur the input with a
    // null relatedTarget between pointerdown and click.
    fireEvent.pointerDown(row);
    act(() => input.blur());
    expect(panel()).not.toBeNull();
    fireEvent.pointerUp(row);
    fireEvent.click(row);
    expect(run).toHaveBeenCalledWith(ACTIONS[0]);
    expect(panel()).toBeNull();
  });

  it("a click that focuses the row keeps the panel open until the row runs", () => {
    const { input } = setup();
    act(() => input.focus());
    const row = screen.getByRole("option", { name: /Theme & density/ });
    fireEvent.pointerDown(row);
    act(() => row.focus());
    expect(panel()).not.toBeNull();
    fireEvent.pointerUp(row);
    fireEvent.click(row);
    expect(run).toHaveBeenCalledWith(ACTIONS[1]);
  });

  it("Escape closes and leaves focus on the input", () => {
    const { input } = setup();
    act(() => input.focus());
    fireEvent.keyDown(input, { key: "Escape" });
    expect(panel()).toBeNull();
    expect(document.activeElement).toBe(input);
  });

  it("Escape from inside the panel closes and returns focus to the input", () => {
    const { input } = setup();
    act(() => input.focus());
    const row = screen.getByRole("option", { name: /Toggle theme/ });
    act(() => row.focus());
    fireEvent.keyDown(row, { key: "Escape" });
    expect(panel()).toBeNull();
    expect(document.activeElement).toBe(input);
  });

  it("an outside pointerdown still closes it", () => {
    const { input, outside } = setup();
    act(() => input.focus());
    fireEvent.pointerDown(outside);
    expect(panel()).toBeNull();
  });
});
