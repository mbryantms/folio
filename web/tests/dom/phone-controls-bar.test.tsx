// @vitest-environment jsdom
/**
 * `<PhoneControlsBar>` — the one-row phone replacement for the desktop
 * grid toolbars. Behavioural invariants every grid page relies on:
 *   - The icon row renders search / sort / filters / ⋯ with accessible
 *     names, and `hidden` renders nothing (the selection toolbar takes
 *     the row's place).
 *   - Search expands in place, keeps keystrokes local (the caller's
 *     value is debounced), and Cancel clears + collapses.
 *   - The sort sheet lists the options as radios, commits + closes on
 *     pick, and the order toggle calls `onOrder`.
 *   - The filters button carries the active count.
 *   - The `⋯` sheet renders `menu(close)` and `close` dismisses it.
 */
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import {
  PhoneControlsBar,
  PhoneMenuAction,
} from "@/components/library/PhoneControlsBar";

const SORT = [
  { value: "name", label: "Name" },
  { value: "year", label: "Year" },
];

describe("PhoneControlsBar", () => {
  it("renders one icon per control and nothing when hidden", () => {
    const { rerender } = render(
      <PhoneControlsBar
        search={{ value: "", onChange: () => {} }}
        sort={{ value: "name", options: SORT, onChange: () => {} }}
        filters={{ count: 0, onOpen: () => {} }}
        menu={() => <p>menu body</p>}
      />,
    );
    expect(screen.getByRole("button", { name: "Search" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Sort: Name" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Filters" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "View options" })).toBeTruthy();

    rerender(
      <PhoneControlsBar
        hidden
        search={{ value: "", onChange: () => {} }}
        menu={() => <p>menu body</p>}
      />,
    );
    expect(screen.queryByRole("button", { name: "Search" })).toBeNull();
  });

  it("expands search in place, keeps keystrokes local, and Cancel clears", () => {
    const onChange = vi.fn();
    const { rerender } = render(
      <PhoneControlsBar
        search={{ value: "", onChange, placeholder: "Search series…" }}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "Search" }));
    const input = screen.getByRole("searchbox", {
      name: "Search series…",
    }) as HTMLInputElement;
    fireEvent.change(input, { target: { value: "bat " } });
    expect(onChange).toHaveBeenLastCalledWith("bat ");
    // The caller echoes back the debounced, trimmed value — the input
    // must keep the trailing space the person is still typing after.
    rerender(
      <PhoneControlsBar
        search={{ value: "bat", onChange, placeholder: "Search series…" }}
      />,
    );
    expect(
      (
        screen.getByRole("searchbox", {
          name: "Search series…",
        }) as HTMLInputElement
      ).value,
    ).toBe("bat ");
    // An external change (Clear filters) does sync.
    rerender(
      <PhoneControlsBar
        search={{ value: "robin", onChange, placeholder: "Search series…" }}
      />,
    );
    expect(
      (
        screen.getByRole("searchbox", {
          name: "Search series…",
        }) as HTMLInputElement
      ).value,
    ).toBe("robin");

    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(onChange).toHaveBeenLastCalledWith("");
    expect(screen.queryByRole("searchbox")).toBeNull();
    expect(screen.getByRole("button", { name: "Search" })).toBeTruthy();
  });

  it("opens the sort sheet, commits a pick, and toggles order", () => {
    const onChange = vi.fn();
    const onOrder = vi.fn();
    render(
      <PhoneControlsBar
        sort={{
          value: "name",
          options: SORT,
          onChange,
          order: "asc",
          onOrder,
        }}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "Sort: Name" }));
    const radios = screen.getAllByRole("radio");
    expect(radios.map((r) => r.getAttribute("aria-checked"))).toEqual([
      "true",
      "false",
    ]);
    fireEvent.click(screen.getByRole("button", { name: "Descending" }));
    expect(onOrder).toHaveBeenCalledWith("desc");
    fireEvent.click(screen.getByRole("radio", { name: "Year" }));
    expect(onChange).toHaveBeenCalledWith("year");
  });

  it("shows the active filter count and opens the filters sheet", () => {
    const onOpen = vi.fn();
    render(<PhoneControlsBar filters={{ count: 3, onOpen }} />);
    const btn = screen.getByRole("button", { name: "Filters (3 active)" });
    expect(btn.getAttribute("aria-pressed")).toBe("true");
    fireEvent.click(btn);
    expect(onOpen).toHaveBeenCalledOnce();
  });

  it("renders the ⋯ sheet body and closes it through `close`", () => {
    const onSelect = vi.fn();
    render(
      <PhoneControlsBar
        menu={(close) => (
          <PhoneMenuAction
            onClick={() => {
              close();
              onSelect();
            }}
          >
            Select
          </PhoneMenuAction>
        )}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "View options" }));
    fireEvent.click(screen.getByRole("button", { name: "Select" }));
    expect(onSelect).toHaveBeenCalledOnce();
  });
});
