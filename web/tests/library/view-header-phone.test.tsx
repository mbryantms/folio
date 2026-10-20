// @vitest-environment jsdom
/**
 * `<ViewHeader>` phone contract (feat/phone-chrome):
 *
 *  - `extraActions` are wrapped in a `max-sm:hidden` box — the saved-view
 *    pages pass the same controls compacted into a `PhoneControlsBar` via
 *    `phoneControls` instead.
 *  - The `⋯` trigger and the `Built-in` chip stay visible on phones (no
 *    `max-sm:hidden` ancestor) — Export / Refresh / Pin live in that menu
 *    and are deliberately *not* duplicated in the phone bar.
 *  - `phoneControls` renders as a *sibling* after the `<header>` element,
 *    never inside it: a `position: sticky` bar inside the header could only
 *    stick while the header's own box is on screen.
 */
import { render } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { SavedViewView } from "@/lib/api/types";

vi.mock("@/lib/api/mutations", () => ({
  useSidebarSavedView: () => ({ mutate: () => {}, isPending: false }),
}));
// Needs a QueryClient + Radix portal; the pin dialog has its own tests.
vi.mock("@/components/saved-views/MultiPinDialog", () => ({
  MultiPinDialog: () => null,
}));

import { ViewHeader } from "@/components/saved-views/ViewHeader";

function view(overrides: Partial<SavedViewView> = {}): SavedViewView {
  return {
    id: "v1",
    kind: "filter_series",
    user_id: "u1",
    is_system: false,
    name: "Horror picks",
    description: "Everything tagged horror, newest first.",
    custom_year_start: null,
    custom_year_end: null,
    custom_tags: [],
    match_mode: "all",
    conditions: [],
    sort_field: "created_at",
    sort_order: "desc",
    result_limit: 12,
    cbl_list_id: null,
    pinned: false,
    pinned_position: null,
    show_in_sidebar: false,
    pinned_on_pages: [],
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
    ...overrides,
  };
}

function hasHiddenOnPhoneAncestor(el: Element | null): boolean {
  return el?.closest(".max-sm\\:hidden") != null;
}

describe("ViewHeader phone layout", () => {
  it("hides extraActions on phones but keeps the ⋯ trigger visible", () => {
    const { container, getByLabelText, getByTestId } = render(
      <ViewHeader
        view={view()}
        extraActions={<button data-testid="desktop-only">Card size</button>}
      />,
    );
    expect(hasHiddenOnPhoneAncestor(getByTestId("desktop-only"))).toBe(true);
    const more = getByLabelText("More actions");
    expect(hasHiddenOnPhoneAncestor(more)).toBe(false);
    // Phone text sizes on the title block.
    const h1 = container.querySelector("h1");
    expect(h1?.className).toContain("max-sm:text-xl");
    const desc = container.querySelector("header p");
    expect(desc?.className).toContain("max-sm:line-clamp-2");
    expect(desc?.className).toContain("max-sm:text-xs");
  });

  it("renders phoneControls as a sibling after <header>, not inside it", () => {
    const { container, getByTestId } = render(
      <ViewHeader
        view={view()}
        extraActions={<span>desktop</span>}
        phoneControls={<div data-testid="phone-bar">bar</div>}
      />,
    );
    const header = container.querySelector("header");
    const bar = getByTestId("phone-bar");
    expect(header).not.toBeNull();
    expect(header!.contains(bar)).toBe(false);
    expect(header!.nextElementSibling).toBe(bar);
    // And it must not inherit the desktop cluster's phone hiding.
    expect(hasHiddenOnPhoneAncestor(bar)).toBe(false);
  });

  it("keeps the Built-in chip in the title row on phones", () => {
    const { getByTitle } = render(
      <ViewHeader
        view={view({ is_system: true, user_id: null })}
        extraActions={<span>desktop</span>}
      />,
    );
    const chip = getByTitle("Built-in views can't be edited or deleted");
    expect(hasHiddenOnPhoneAncestor(chip)).toBe(false);
  });

  it('places phoneControls inside the title row with placement="inline"', () => {
    const { container, getByTestId } = render(
      <ViewHeader
        view={view()}
        extraActions={<span>desktop</span>}
        phoneControls={<div data-testid="phone-bar">bar</div>}
        phoneControlsPlacement="inline"
      />,
    );
    const header = container.querySelector("header");
    const bar = getByTestId("phone-bar");
    expect(header!.contains(bar)).toBe(true);
    // Inline means next to the ⋯ trigger, not inside the hidden desktop
    // cluster.
    expect(hasHiddenOnPhoneAncestor(bar)).toBe(false);
    expect(bar.closest("[data-view-header-actions]")).toBeNull();
    expect(header!.nextElementSibling).toBeNull();
  });

  it("does not render the hidden wrapper when there are no extraActions", () => {
    const { container } = render(<ViewHeader view={view()} />);
    expect(container.querySelector("[data-view-header-actions]")).toBeNull();
  });
});
