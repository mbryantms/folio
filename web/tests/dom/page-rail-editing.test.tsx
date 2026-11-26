// @vitest-environment jsdom
/**
 * Editing a page's rails on the page (inline rail editing): the rail ⋯
 * menu and Rearrange mode both save the whole order for the page, and a
 * removal offers Undo that re-pins the rail in its old place.
 */
import { fireEvent, render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { SavedViewView } from "@/lib/api/types";

const reorder = vi.fn();
const reorderAsync = vi.fn(async () => ({}));
const toggle = vi.fn();
const toggleAsync = vi.fn(async () => ({}));
const undo: { onUndo?: () => unknown; message?: string } = {};

vi.mock("@/lib/api/mutations", () => ({
  useReorderSavedViews: () => ({ mutate: reorder, mutateAsync: reorderAsync }),
  useTogglePinOnPage: () => ({ mutate: toggle, mutateAsync: toggleAsync }),
}));
// The springy drag reads the reader's motion preference.
vi.mock("@/lib/visual-effects", () => ({
  useVisualEffect: () => false,
  useReducedMotion: () => true,
}));
vi.mock("@/lib/ui/undo-toast", () => ({
  showUndoToast: (opts: { onUndo: () => unknown; message: string }) => {
    undo.onUndo = opts.onUndo;
    undo.message = opts.message;
  },
}));
vi.mock("@/components/saved-views/ManagePinsDialog", () => ({
  ManagePinsDialog: ({ open }: { open: boolean }) =>
    open ? <div role="dialog">Add or remove rails</div> : null,
}));
vi.mock("@/components/saved-views/SavedViewRail", () => ({
  viewDetailHref: (v: { id: string }) => `/views/${v.id}`,
}));

import {
  RailMenu,
  RearrangeRails,
  usePageRailEdit,
} from "@/components/saved-views/PageRailEditing";

function view(id: string): SavedViewView {
  return { id, name: `Rail ${id}`, kind: "filter_series" } as SavedViewView;
}
const RAILS = [view("a"), view("b"), view("c")];

/** Open a rail's ⋯ menu (Radix opens on pointer down). */
function openMenu(rail: string) {
  fireEvent.pointerDown(
    screen.getByRole("button", { name: `${rail} rail actions` }),
    { button: 0, ctrlKey: false },
  );
}

function Menus() {
  const edit = usePageRailEdit("page-1", RAILS);
  return (
    <>
      {edit.ordered.map((v, i) => (
        <RailMenu
          key={v.id}
          view={v}
          index={i}
          count={edit.ordered.length}
          edit={edit}
        />
      ))}
    </>
  );
}

function Rearrange({ onDone }: { onDone: () => void }) {
  const edit = usePageRailEdit("page-1", RAILS);
  return <RearrangeRails pageId="page-1" edit={edit} onDone={onDone} />;
}

beforeEach(() => {
  reorder.mockClear();
  reorderAsync.mockClear();
  toggle.mockClear();
  toggleAsync.mockClear();
  undo.onUndo = undefined;
});

describe("RailMenu", () => {
  it("moves a rail and saves the page's whole order", async () => {
    render(<Menus />);
    openMenu("Rail b");
    fireEvent.click(screen.getByRole("menuitem", { name: "Move up" }));
    expect(reorder.mock.calls[0]![0]).toEqual({
      page_id: "page-1",
      view_ids: ["b", "a", "c"],
    });
  });

  it("cannot move the first rail up or the last one down", async () => {
    render(<Menus />);
    openMenu("Rail a");
    expect(
      screen
        .getByRole("menuitem", { name: "Move up" })
        .getAttribute("aria-disabled"),
    ).toBe("true");
    expect(
      screen.getByRole("menuitem", { name: "Open view" }).getAttribute("href"),
    ).toBe("/views/a");
  });

  it("removes a rail with an Undo that re-pins it in place", async () => {
    render(<Menus />);
    openMenu("Rail b");
    fireEvent.click(screen.getByRole("menuitem", { name: "Remove from page" }));
    expect(toggle.mock.calls[0]![0]).toEqual({
      viewId: "b",
      pageId: "page-1",
      pinned: false,
    });
    // The mutation reports success, which raises the Undo toast.
    (toggle.mock.calls[0]![1] as { onSuccess: () => void }).onSuccess();
    expect(undo.message).toBe('Removed "Rail b" from this page');
    await undo.onUndo?.();
    expect(toggleAsync).toHaveBeenCalledWith({
      viewId: "b",
      pageId: "page-1",
      pinned: true,
    });
    expect(reorderAsync).toHaveBeenCalledWith({
      page_id: "page-1",
      view_ids: ["a", "b", "c"],
    });
  });
});

describe("RearrangeRails", () => {
  it("lists every rail with arrows, a grip and remove", () => {
    render(<Rearrange onDone={() => {}} />);
    const rows = within(screen.getByRole("list")).getAllByRole("listitem");
    expect(rows.map((r) => r.textContent)).toEqual([
      "1Rail a",
      "2Rail b",
      "3Rail c",
    ]);
    expect(
      (
        screen.getByRole("button", {
          name: "Move Rail a up",
        }) as HTMLButtonElement
      ).disabled,
    ).toBe(true);
    expect(
      (
        screen.getByRole("button", {
          name: "Move Rail c down",
        }) as HTMLButtonElement
      ).disabled,
    ).toBe(true);
    // A finger on the grip drags; it must not scroll the page.
    expect(
      screen.getByRole("button", { name: "Drag Rail a" }).className,
    ).toContain("touch-none");
  });

  it("steps a rail down, opens Add rail, and reports Done", () => {
    const onDone = vi.fn();
    render(<Rearrange onDone={onDone} />);
    fireEvent.click(screen.getByRole("button", { name: "Move Rail a down" }));
    expect(reorder.mock.calls[0]![0]).toEqual({
      page_id: "page-1",
      view_ids: ["b", "a", "c"],
    });
    fireEvent.click(screen.getByRole("button", { name: "Add rail" }));
    expect(screen.getByRole("dialog")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Done" }));
    expect(onDone).toHaveBeenCalled();
  });
});
