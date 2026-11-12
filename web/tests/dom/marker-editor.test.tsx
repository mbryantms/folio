// @vitest-environment jsdom
/**
 * Hydrated render tests for the marker editor sheet: validation before
 * mutation, the tag-chip input, and the dirty-cancel Undo toast. The API
 * hooks are mocked at the module seam (mutations never hit the network).
 */
import * as React from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type * as Queries from "@/lib/api/queries";
import type * as Mutations from "@/lib/api/mutations/markers";

// vi.mock factories are hoisted above imports, so anything they close over
// must be hoisted too.
const { PERSISTED } = vi.hoisted(() => ({
  PERSISTED: {
    id: "m-del",
    user_id: "u-1",
    series_id: "s-1",
    issue_id: "issue-1",
    page_index: 2,
    kind: "note",
    is_favorite: false,
    tags: ["plot"],
    region: null,
    selection: null,
    body: "keep me",
    color: "violet",
    page_hash: "b".repeat(64),
    hidden_from_log: false,
    created_at: "2026-09-01T10:00:00+00:00",
    updated_at: "2026-09-01T10:00:00+00:00",
  },
}));
const {
  toast,
  createMutate,
  updateMutate,
  deleteMutate,
  restoreMutate,
  apiFetch,
} = vi.hoisted(() => ({
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
  updateMutate: vi.fn(),
  deleteMutate: vi.fn(),
  restoreMutate: vi.fn(),
  apiFetch: vi.fn(),
}));
vi.mock("sonner", () => ({ toast }));
// The OCR request is asserted at the network seam (its body).
vi.mock("@/lib/api/auth-refresh", () => ({
  apiFetch,
  getCsrfToken: () => "csrf",
}));

vi.mock("@/lib/api/queries", async (importOriginal) => ({
  ...(await importOriginal<typeof Queries>()),
  useMarkerTags: () => ({
    data: { items: [{ tag: "action" }, { tag: "adventure" }] },
  }),
  // The persisted rows the overlay already holds (Undo restores from it).
  useIssueMarkers: () => ({ data: { items: [PERSISTED] } }),
}));
vi.mock("@/lib/api/mutations/markers", async (importOriginal) => ({
  ...(await importOriginal<typeof Mutations>()),
  useCreateMarker: () => ({
    mutate: createMutate,
    mutateAsync: createMutate,
    isPending: false,
  }),
  useUpdateMarker: () => ({ mutate: updateMutate, isPending: false }),
  useDeleteMarker: () => ({ mutate: deleteMutate, isPending: false }),
  useRestoreMarkers: () => ({
    mutate: restoreMutate,
    mutateAsync: restoreMutate,
    isPending: false,
  }),
}));

import { MarkerEditor } from "@/app/[locale]/read/[seriesSlug]/[issueSlug]/MarkerEditor";
import { useReaderStore } from "@/lib/reader/store";

function openNote(body = "") {
  useReaderStore.getState().beginMarkerEdit(
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

/** Page 2 is 1988 x 3056 on the server. */
const SERVER_SIZE = { width: 1988, height: 3056 };

function renderEditor() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <MarkerEditor
        issueId="issue-1"
        pageSizeSources={() => ({ server: SERVER_SIZE, measured: null })}
      />
    </QueryClientProvider>,
  );
}

function ocrReplies(...responses: Response[]) {
  for (const r of responses) apiFetch.mockResolvedValueOnce(r);
}
function json(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });
}
function ocrBodies() {
  return apiFetch.mock.calls.map(
    ([, init]) => JSON.parse((init as RequestInit).body as string) as unknown,
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
  deleteMutate.mockReset();
  restoreMutate.mockReset();
  createMutate.mockReset();
  updateMutate.mockReset();
  toast.mockReset();
  toast.error.mockReset();
  toast.success.mockReset();
  toast.message.mockReset();
  apiFetch.mockReset();
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

  it("delete offers an Undo that restores the persisted row in one request (WP-8.4)", async () => {
    useReaderStore.getState().beginMarkerEdit(
      {
        kind: "note",
        page_index: 2,
        region: null,
        selection: null,
        body: "keep me",
        is_favorite: false,
        tags: ["plot"],
      },
      "m-del",
    );
    renderEditor();
    fireEvent.click(await screen.findByRole("button", { name: /delete/i }));
    await waitFor(() => expect(deleteMutate).toHaveBeenCalledTimes(1));
    // Run the delete's onSuccess, then click the toast's Undo.
    deleteMutate.mock.calls[0]?.[1]?.onSuccess?.();
    await waitFor(() =>
      expect(toast.success).toHaveBeenCalledWith(
        "Removed",
        expect.objectContaining({
          action: expect.objectContaining({ label: "Undo" }),
        }),
      ),
    );
    const undo = toast.success.mock.calls[0]?.[1]?.action as {
      onClick: () => void;
    };
    undo.onClick();
    expect(restoreMutate).toHaveBeenCalledTimes(1);
    expect(restoreMutate).toHaveBeenCalledWith([PERSISTED]);
    expect(createMutate).not.toHaveBeenCalled();
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

  describe("captured text (WP-5.3)", () => {
    function openHighlight(
      selection: {
        text?: string;
        ocr_confidence?: number;
        image_hash?: string;
      } | null,
      editingId: string | null,
    ) {
      useReaderStore.getState().beginMarkerEdit(
        {
          kind: "highlight",
          page_index: 2,
          region: { x: 1, y: 1, w: 20, h: 10, shape: "text" },
          selection,
          body: "caption",
          is_favorite: false,
          tags: [],
        },
        editingId,
      );
    }

    it("round-trips a hand-corrected OCR text on an existing marker", async () => {
      openHighlight(
        { text: "WE ARE B0RN", ocr_confidence: 0.4, image_hash: "h1" },
        "m-1",
      );
      renderEditor();
      const field = (await screen.findByLabelText(
        "Captured text",
      )) as HTMLTextAreaElement;
      expect(field.value).toBe("WE ARE B0RN");
      expect(screen.getByText(/low confidence/i)).toBeTruthy();

      fireEvent.change(field, { target: { value: "We are born" } });
      // A hand edit drops the OCR confidence (and its warning).
      expect(screen.queryByText(/low confidence/i)).toBeNull();
      fireEvent.click(screen.getByRole("button", { name: "Save" }));

      await waitFor(() => expect(updateMutate).toHaveBeenCalledTimes(1));
      expect(updateMutate.mock.calls[0]?.[0]).toEqual({
        body: "caption",
        is_favorite: false,
        tags: [],
        selection: { text: "We are born", image_hash: "h1" },
      });
    });

    it("does not resend the selection when only the caption changed", async () => {
      openHighlight({ text: "original", ocr_confidence: 0.9 }, "m-2");
      renderEditor();
      fireEvent.change(await screen.findByLabelText("Caption"), {
        target: { value: "new caption" },
      });
      fireEvent.click(screen.getByRole("button", { name: "Save" }));
      await waitFor(() => expect(updateMutate).toHaveBeenCalledTimes(1));
      expect(updateMutate.mock.calls[0]?.[0]).not.toHaveProperty("selection");
    });

    it("lets a new highlight's text be typed by hand", async () => {
      openHighlight(null, null);
      renderEditor();
      expect(screen.queryByLabelText("Captured text")).toBeNull();
      fireEvent.click(
        await screen.findByRole("button", { name: /type text/i }),
      );
      fireEvent.change(screen.getByLabelText("Captured text"), {
        target: { value: "typed line" },
      });
      fireEvent.click(screen.getByRole("button", { name: "Save marker" }));
      await waitFor(() => expect(createMutate).toHaveBeenCalledTimes(1));
      expect(createMutate.mock.calls[0]?.[0]).toMatchObject({
        kind: "highlight",
        selection: { text: "typed line" },
      });
    });

    it("refuses captured text over 8 KB before any mutation", async () => {
      openHighlight({ text: "short" }, "m-3");
      renderEditor();
      fireEvent.change(await screen.findByLabelText("Captured text"), {
        target: { value: "x".repeat(8 * 1024 + 1) },
      });
      fireEvent.click(screen.getByRole("button", { name: "Save" }));
      expect(toast.error).toHaveBeenCalledWith(
        "Captured text is too long (max 8 KB).",
      );
      expect(updateMutate).not.toHaveBeenCalled();
    });

    it("Detect text sends the region in the page's original pixels (D1)", async () => {
      ocrReplies(json(200, { text: "AGAIN", confidence: 0.9 }));
      openHighlight({ text: "OLD", ocr_confidence: 0.9 }, "m-4");
      renderEditor();
      fireEvent.click(
        await screen.findByRole("button", { name: /re-detect text/i }),
      );
      await waitFor(() => expect(apiFetch).toHaveBeenCalledTimes(1));
      // 1% / 20% / 10% of 1988 x 3056 — not of a variant's natural size.
      expect(ocrBodies()[0]).toEqual({
        page: 2,
        region: { x: 20, y: 31, w: 398, h: 306 },
      });
    });

    it("after a Re-detect that saved, the editor is not dirty for that text (D7)", async () => {
      ocrReplies(json(200, { text: "FRESH TEXT", confidence: 0.95 }));
      openHighlight(
        { text: "OLD", ocr_confidence: 0.9, image_hash: "h" },
        "m-5",
      );
      renderEditor();
      fireEvent.click(
        await screen.findByRole("button", { name: /re-detect text/i }),
      );
      await waitFor(() => expect(updateMutate).toHaveBeenCalledTimes(1));
      // Saved at once, keeping the other selection fields.
      expect(updateMutate.mock.calls[0]?.[0]).toEqual({
        selection: {
          text: "FRESH TEXT",
          ocr_confidence: 0.95,
          image_hash: "h",
        },
      });
      // The patch lands.
      React.act(() => updateMutate.mock.calls[0]?.[1]?.onSuccess?.());
      expect(
        (screen.getByLabelText("Captured text") as HTMLTextAreaElement).value,
      ).toBe("FRESH TEXT");

      // Save does not send the saved text again …
      fireEvent.click(screen.getByRole("button", { name: "Save" }));
      await waitFor(() => expect(updateMutate).toHaveBeenCalledTimes(2));
      expect(updateMutate.mock.calls[1]?.[0]).not.toHaveProperty("selection");
    });

    it("Cancel after a Re-detect that saved closes without 'Discarded unsaved changes' (D7)", async () => {
      ocrReplies(json(200, { text: "FRESH TEXT", confidence: 0.95 }));
      openHighlight({ text: "OLD", ocr_confidence: 0.9 }, "m-6");
      renderEditor();
      fireEvent.click(
        await screen.findByRole("button", { name: /re-detect text/i }),
      );
      await waitFor(() => expect(updateMutate).toHaveBeenCalledTimes(1));
      React.act(() => updateMutate.mock.calls[0]?.[1]?.onSuccess?.());
      fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
      expect(useReaderStore.getState().pendingMarker).toBeNull();
      expect(toast).not.toHaveBeenCalledWith(
        "Discarded unsaved changes",
        expect.anything(),
      );
    });

    it("asks before Re-detect replaces hand-edited text, and not before replacing recognized text (D7)", async () => {
      openHighlight({ text: "OCR TEXT", ocr_confidence: 0.9 }, "m-7");
      renderEditor();
      const field = (await screen.findByLabelText(
        "Captured text",
      )) as HTMLTextAreaElement;
      fireEvent.change(field, { target: { value: "My corrected text" } });
      fireEvent.click(screen.getByRole("button", { name: /re-detect text/i }));
      const dialog = await screen.findByRole("alertdialog");
      expect(dialog.textContent).toContain("Replace the text you edited?");
      expect(apiFetch).not.toHaveBeenCalled();
      // Keep: nothing runs, the text stays.
      fireEvent.click(screen.getByRole("button", { name: "Keep my text" }));
      await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
      expect(apiFetch).not.toHaveBeenCalled();
      expect(field.value).toBe("My corrected text");

      // Replace: now it reads.
      ocrReplies(json(200, { text: "OCR TEXT 2", confidence: 0.9 }));
      fireEvent.click(screen.getByRole("button", { name: /re-detect text/i }));
      fireEvent.click(
        await screen.findByRole("button", { name: "Replace text" }),
      );
      await waitFor(() => expect(apiFetch).toHaveBeenCalledTimes(1));
    });

    it("Clear on recognized text runs at once; on hand-typed text it asks (D7)", async () => {
      openHighlight({ text: "OCR TEXT", ocr_confidence: 0.9 }, null);
      renderEditor();
      fireEvent.click(await screen.findByRole("button", { name: "Clear" }));
      expect(screen.queryByRole("alertdialog")).toBeNull();
      expect(screen.queryByLabelText("Captured text")).toBeNull();

      // Stored text without a confidence was written by hand.
      useReaderStore.getState().beginMarkerEdit(null, null);
      openHighlight({ text: "typed by me" }, null);
      fireEvent.click(await screen.findByRole("button", { name: "Clear" }));
      expect((await screen.findByRole("alertdialog")).textContent).toContain(
        "Clear the text you edited?",
      );
    });

    it("says why Detect text failed, by the server's error code (D5)", async () => {
      ocrReplies(
        json(409, {
          error: { code: "archive_needs_conversion", message: "convert" },
        }),
      );
      openHighlight(null, "m-8");
      renderEditor();
      fireEvent.click(
        await screen.findByRole("button", { name: /detect text/i }),
      );
      await waitFor(() => expect(toast.error).toHaveBeenCalledTimes(1));
      expect(toast.error.mock.calls[0]?.[0]).toBe(
        "Couldn't read the text. This issue's file has to be converted to CBZ before its text can be read.",
      );
      expect(updateMutate).not.toHaveBeenCalled();
    });
  });
});
