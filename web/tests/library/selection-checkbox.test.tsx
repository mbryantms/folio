/**
 * Regression guard for audit finding E9: out of select mode the
 * `SelectionCheckbox` sits inside a card's `<Link>` (anchor), so it must
 * NOT be a real `<button>` (interactive content nested in interactive
 * content is invalid HTML + a React 19 hydration warning). It renders a
 * focusable `<span role="checkbox" tabIndex={0}>` instead. In select
 * mode the whole card is the button, so the checkbox is a decorative
 * `aria-hidden` span.
 *
 * Vitest runs node-env without a DOM, so we call the component function
 * directly and inspect the returned React element (same style as
 * `quick-read-overlay.test.tsx`).
 */
import { describe, expect, it, vi } from "vitest";
import type * as React from "react";

import { SelectionCheckbox } from "@/components/library/SelectionCheckbox";

type SpanProps = {
  role?: string;
  tabIndex?: number;
  "aria-checked"?: boolean;
  "aria-hidden"?: string | boolean;
  "aria-label"?: string;
  onClick?: (e: React.MouseEvent) => void;
  onKeyDown?: (e: React.KeyboardEvent) => void;
};

describe("SelectionCheckbox (E9 — no button-in-anchor)", () => {
  it("renders a focusable role=checkbox SPAN out of select mode", () => {
    const tree = SelectionCheckbox({
      isSelected: false,
      selectMode: false,
      onToggle: () => {},
      label: "Saga #1",
    }) as React.ReactElement<SpanProps>;

    // The crux of E9: a span, never a <button>, so it's valid inside <a>.
    expect(tree.type).toBe("span");
    expect(tree.props.role).toBe("checkbox");
    expect(tree.props.tabIndex).toBe(0);
    expect(tree.props["aria-checked"]).toBe(false);
    expect(tree.props["aria-label"]).toBe("Select Saga #1");
  });

  it("announces the selected state via aria-checked + Deselect label", () => {
    const tree = SelectionCheckbox({
      isSelected: true,
      selectMode: false,
      onToggle: () => {},
      label: "Saga #1",
    }) as React.ReactElement<SpanProps>;

    expect(tree.props["aria-checked"]).toBe(true);
    expect(tree.props["aria-label"]).toBe("Deselect Saga #1");
  });

  it("click toggles, stops propagation, and prevents the anchor navigating", () => {
    const onToggle = vi.fn();
    const tree = SelectionCheckbox({
      isSelected: false,
      selectMode: false,
      onToggle,
      label: "Saga #1",
    }) as React.ReactElement<SpanProps>;

    const stopPropagation = vi.fn();
    const preventDefault = vi.fn();
    tree.props.onClick?.({
      stopPropagation,
      preventDefault,
    } as unknown as React.MouseEvent);

    expect(onToggle).toHaveBeenCalledTimes(1);
    expect(stopPropagation).toHaveBeenCalledTimes(1);
    expect(preventDefault).toHaveBeenCalledTimes(1);
  });

  it("Space and Enter activate the checkbox", () => {
    for (const key of [" ", "Enter"]) {
      const onToggle = vi.fn();
      const tree = SelectionCheckbox({
        isSelected: false,
        selectMode: false,
        onToggle,
        label: "Saga #1",
      }) as React.ReactElement<SpanProps>;
      tree.props.onKeyDown?.({
        key,
        stopPropagation: () => {},
        preventDefault: () => {},
      } as unknown as React.KeyboardEvent);
      expect(onToggle).toHaveBeenCalledTimes(1);
    }
  });

  it("is a decorative aria-hidden span in select mode (card owns the click)", () => {
    const tree = SelectionCheckbox({
      isSelected: true,
      selectMode: true,
      onToggle: () => {},
      label: "Saga #1",
    }) as React.ReactElement<SpanProps>;

    expect(tree.type).toBe("span");
    expect(tree.props["aria-hidden"]).toBe("true");
    // No interactive role/handlers — the parent card-button drives it.
    expect(tree.props.role).toBeUndefined();
    expect(tree.props.onClick).toBeUndefined();
  });
});
