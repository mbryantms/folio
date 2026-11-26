// @vitest-environment jsdom
/**
 * The in-place sidebar editor: it draws the layout in sidebar order with
 * headers as section labels, and every edit saves the whole list with the
 * hidden rows kept.
 */
import { fireEvent, render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { SidebarEntryView, UpdateEntryReq } from "@/lib/api/types";

const mutate = vi.fn();
const state = vi.hoisted(() => ({ entries: [] as unknown[] }));

vi.mock("@/lib/api/queries", () => ({
  useSidebarLayout: () => ({
    data: { entries: state.entries },
    isLoading: false,
    isError: false,
  }),
}));
vi.mock("@/lib/api/mutations", () => ({
  useUpdateSidebarLayout: () => ({ mutate, mutateAsync: vi.fn() }),
}));
vi.mock("@/lib/visual-effects", () => ({
  useVisualEffect: () => false,
  useReducedMotion: () => true,
}));

import { SidebarEditMode } from "@/components/library/SidebarEditMode";

function row(
  kind: SidebarEntryView["kind"],
  ref_id: string,
  label: string,
  visible = true,
): SidebarEntryView {
  return {
    kind,
    ref_id,
    label,
    icon: "",
    href: `/${ref_id}`,
    visible,
    position: 0,
  };
}

const sent = () =>
  (mutate.mock.calls.at(-1)![0] as { entries: UpdateEntryReq[] }).entries;
const order = () =>
  sent()
    .filter((e) => e.visible)
    .map((e) => e.ref_id);

function openOptions(name: string) {
  fireEvent.pointerDown(
    screen.getByRole("button", { name: `${name} options` }),
    {
      button: 0,
      ctrlKey: false,
    },
  );
}

beforeEach(() => {
  mutate.mockClear();
  state.entries = [
    row("header", "default:browse", "Browse"),
    row("builtin", "home", "Home"),
    row("builtin", "bookmarks", "Bookmarks"),
    row("header", "h-custom", "Mine"),
    row("view", "v1", "Hickman"),
    row("builtin", "want_to_read", "Want to Read", false),
  ];
});

describe("SidebarEditMode", () => {
  it("lists the sidebar in order, with hidden items below", () => {
    render(<SidebarEditMode onDone={() => {}} />);
    const rows = within(screen.getAllByRole("list")[0]!).getAllByRole(
      "listitem",
    );
    expect(rows.map((r) => r.getAttribute("data-kind"))).toEqual([
      "header",
      "builtin",
      "builtin",
      "header",
      "view",
    ]);
    expect(screen.getByText("Hidden")).toBeTruthy();
    expect(
      screen.getByRole("button", { name: "Show Want to Read in the sidebar" }),
    ).toBeTruthy();
    // A finger on the grip drags instead of scrolling the sheet.
    expect(
      screen.getByRole("button", { name: "Drag Home" }).className,
    ).toContain("touch-none");
  });

  it("moves a row and keeps the hidden rows in what it saves", () => {
    render(<SidebarEditMode onDone={() => {}} />);
    openOptions("Home");
    fireEvent.click(screen.getByRole("menuitem", { name: "Move down" }));
    expect(order()).toEqual([
      "default:browse",
      "bookmarks",
      "home",
      "h-custom",
      "v1",
    ]);
    expect(sent().at(-1)).toMatchObject({
      ref_id: "want_to_read",
      visible: false,
    });
  });

  it("hides a row, shows a hidden one, and adds a header above a row", () => {
    render(<SidebarEditMode onDone={() => {}} />);
    openOptions("Bookmarks");
    fireEvent.click(
      screen.getByRole("menuitem", { name: "Hide from sidebar" }),
    );
    expect(order()).not.toContain("bookmarks");
    expect(sent().find((e) => e.ref_id === "bookmarks")?.visible).toBe(false);

    fireEvent.click(
      screen.getByRole("button", { name: "Show Want to Read in the sidebar" }),
    );
    expect(order().at(-1)).toBe("want_to_read");

    openOptions("Hickman");
    fireEvent.click(screen.getByRole("menuitem", { name: "Add header above" }));
    const headers = sent().filter((e) => e.kind === "header");
    expect(headers.map((h) => h.label)).toEqual([
      "Browse",
      "Mine",
      "New header",
    ]);
    const ids = order();
    expect(ids.indexOf(headers[2]!.ref_id)).toBe(ids.indexOf("v1") - 1);
  });

  it("offers Remove for a custom header and Hide for a default one", () => {
    render(<SidebarEditMode onDone={() => {}} />);
    openOptions("Mine");
    expect(
      screen.getByRole("menuitem", { name: "Remove header" }),
    ).toBeTruthy();
    fireEvent.click(screen.getByRole("menuitem", { name: "Remove header" }));
    expect(sent().some((e) => e.ref_id === "h-custom")).toBe(false);
  });

  it("renames in place on Enter", () => {
    render(<SidebarEditMode onDone={() => {}} />);
    openOptions("Hickman");
    fireEvent.click(screen.getByRole("menuitem", { name: "Rename" }));
    const input = screen.getByRole("textbox", { name: "Rename Hickman" });
    fireEvent.change(input, { target: { value: "Hickman run" } });
    fireEvent.keyDown(input, { key: "Enter" });
    expect(sent().find((e) => e.ref_id === "v1")?.label).toBe("Hickman run");
  });

  it("reports Done", () => {
    const onDone = vi.fn();
    render(<SidebarEditMode onDone={onDone} />);
    fireEvent.click(screen.getByRole("button", { name: "Done" }));
    expect(onDone).toHaveBeenCalled();
  });
});
