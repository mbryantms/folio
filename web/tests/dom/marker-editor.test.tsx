// @vitest-environment jsdom
/**
 * Hydrated render tests for the marker editor sheet: validation before
 * mutation, the tag-chip input, and the dirty-cancel Undo toast. The API
 * hooks are mocked at the module seam (mutations never hit the network).
 */
import * as React from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import type * as Queries from "@/lib/api/queries";
import type * as Mutations from "@/lib/api/mutations";

// vi.mock factories are hoisted above imports, so anything they close over
// must be hoisted too.
const { toast, createMutate } = vi.hoisted(() => ({
  toast: Object.assign(vi.fn(), {
    error: vi.fn(),
    success: vi.fn(),
    loading: vi.fn(),
    dismiss: vi.fn(),
    message: vi.fn(),
    info: vi.fn(),
    warning: vi.fn(),
  }),
  createMutate: vi.fn(),
}));
vi.mock("sonner", () => ({ toast }));

vi.mock("@/lib/api/queries", async (importOriginal) => ({
  ...(await importOriginal<typeof Queries>()),
  useMarkerTags: () => ({
    data: { items: [{ tag: "action" }, { tag: "adventure" }] },
  }),
}));
vi.mock("@/lib/api/mutations", async (importOriginal) => ({
  ...(await importOriginal<typeof Mutations>()),
  useCreateMarker: () => ({ mutate: createMutate, isPending: false }),
  useUpdateMarker: () => ({ mutate: vi.fn(), isPending: false }),
  useDeleteMarker: () => ({ mutate: vi.fn(), isPending: false }),
}));

import { MarkerEditor } from "@/app/[locale]/read/[seriesSlug]/[issueSlug]/MarkerEditor";
import { useReaderStore } from "@/lib/reader/store";

function openNote(body = "") {
  useReaderStore
    .getState()
    .beginMarkerEdit(
      {
        kind: "note",
        page_index: 0,
        region: null,
        selection: null,
        body,
        is_favorite: false,
        tags: [],
      },
      null,
    );
}

function renderEditor() {
  const sizes = {
    current: new Map<number, { width: number; height: number }>(),
  };
  return render(
    <MarkerEditor issueId="issue-1" pageNaturalSize={sizes as never} />,
  );
}

beforeEach(() => {
  useReaderStore.setState({
    pendingMarker: null,
    editingMarkerId: null,
    chromePinned: false,
  } as never);
});
afterEach(() => {
  createMutate.mockReset();
  toast.mockReset();
  toast.error.mockReset();
});

describe("MarkerEditor (jsdom)", () => {
  it("renders nothing until a pending marker exists", () => {
    const { container } = renderEditor();
    expect(container.innerHTML).toBe("");
  });

  it("rejects an empty note body before any mutation, then submits the typed body", async () => {
    openNote();
    renderEditor();
    expect(await screen.findByText("Add note")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Save marker" }));
    expect(toast.error).toHaveBeenCalledWith(
      "Notes need a body — type something or pick another kind.",
    );
    expect(createMutate).not.toHaveBeenCalled();

    fireEvent.change(screen.getByLabelText("Note"), {
      target: { value: "remember this" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save marker" }));
    await waitFor(() => expect(createMutate).toHaveBeenCalledTimes(1));
    expect(createMutate.mock.calls[0]?.[0]).toMatchObject({
      issue_id: "issue-1",
      page_index: 0,
      kind: "note",
      body: "remember this",
      is_favorite: false,
      tags: [],
    });
  });

  it("tag input: Enter adds a lowercased chip, duplicates are ignored, Backspace removes", async () => {
    openNote("x");
    renderEditor();
    const input = (await screen.findByLabelText("Tags")) as HTMLInputElement;

    fireEvent.change(input, { target: { value: "Action" } });
    fireEvent.keyDown(input, { key: "Enter" });
    expect(screen.getByRole("button", { name: "Remove action" })).toBeTruthy();

    fireEvent.change(input, { target: { value: "action" } });
    fireEvent.keyDown(input, { key: "Enter" });
    expect(
      screen.getAllByRole("button", { name: "Remove action" }),
    ).toHaveLength(1);

    // Autocomplete never re-suggests an applied tag.
    fireEvent.change(input, { target: { value: "a" } });
    expect(screen.getByRole("button", { name: "adventure" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: /^action$/ })).toBeNull();

    fireEvent.change(input, { target: { value: "" } });
    fireEvent.keyDown(input, { key: "Backspace" });
    expect(screen.queryByRole("button", { name: "Remove action" })).toBeNull();
  });

  it("cancel on a dirty draft closes the sheet and offers Undo; pristine cancel is silent", async () => {
    openNote();
    renderEditor();
    fireEvent.change(await screen.findByLabelText("Note"), {
      target: { value: "hello" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(useReaderStore.getState().pendingMarker).toBeNull();
    expect(toast).toHaveBeenCalledWith(
      "Discarded unsaved changes",
      expect.objectContaining({
        action: expect.objectContaining({ label: "Undo" }),
      }),
    );

    toast.mockReset();
    openNote();
    fireEvent.click(await screen.findByRole("button", { name: "Cancel" }));
    expect(useReaderStore.getState().pendingMarker).toBeNull();
    expect(toast).not.toHaveBeenCalled();
  });
});
