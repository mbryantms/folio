/**
 * Markers + Collections M5 — reader-store marker state + selection
 * helpers. The DOM-side overlay is exercised manually via the dev
 * server (vitest runs in node env with no canvas), but the math + state
 * machine that drive it live here.
 */
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { useReaderStore } from "@/lib/reader/store";
import {
  KEYBIND_DEFAULTS,
  actionForKey,
  resolveKeybinds,
} from "@/lib/reader/keybinds";

function resetStore() {
  useReaderStore.setState({
    issueId: "",
    seriesId: null,
    currentPage: 0,
    totalPages: 0,
    chromeVisible: true,
    chromeAutoHide: true,
    chromePinned: false,
    pageStripVisible: false,
    brightness: 1,
    sepia: 0,
    coverSolo: true,
    markerMode: "idle",
    pendingMarker: null,
    editingMarkerId: null,
  });
}

describe("reader store — marker state", () => {
  beforeEach(() => {
    resetStore();
  });

  it("starts in idle mode with no pending marker", () => {
    const s = useReaderStore.getState();
    expect(s.markerMode).toBe("idle");
    expect(s.pendingMarker).toBeNull();
    expect(s.editingMarkerId).toBeNull();
  });

  it("setMarkerMode flips the mode without touching pendingMarker", () => {
    const { setMarkerMode } = useReaderStore.getState();
    setMarkerMode("select-rect");
    expect(useReaderStore.getState().markerMode).toBe("select-rect");
    expect(useReaderStore.getState().pendingMarker).toBeNull();
  });

  it("beginMarkerEdit stores the pending sketch and resets mode to idle when cleared", () => {
    const { beginMarkerEdit, setMarkerMode } = useReaderStore.getState();
    setMarkerMode("select-rect");
    beginMarkerEdit({
      kind: "highlight",
      page_index: 3,
      region: { x: 10, y: 20, w: 30, h: 15, shape: "rect" },
      selection: null,
      body: "",
      is_favorite: false,
      tags: [],
    });
    const open = useReaderStore.getState();
    expect(open.pendingMarker?.kind).toBe("highlight");
    expect(open.pendingMarker?.page_index).toBe(3);
    // Mode stays at the current `select-*` so the overlay can keep
    // capturing pointer events until the user commits in the editor.
    expect(open.markerMode).toBe("select-rect");

    beginMarkerEdit(null, null);
    const closed = useReaderStore.getState();
    expect(closed.pendingMarker).toBeNull();
    expect(closed.editingMarkerId).toBeNull();
    // Closing the editor always returns mode to idle (ESC + save both
    // funnel through here).
    expect(closed.markerMode).toBe("idle");
  });

  it("editing an existing marker tracks the id alongside the pending sketch", () => {
    const { beginMarkerEdit } = useReaderStore.getState();
    beginMarkerEdit(
      {
        kind: "note",
        page_index: 0,
        region: null,
        selection: null,
        body: "Existing body",
        is_favorite: false,
        tags: [],
      },
      "marker-id-123",
    );
    const s = useReaderStore.getState();
    expect(s.editingMarkerId).toBe("marker-id-123");
    expect(s.pendingMarker?.body).toBe("Existing body");
  });

  it("init() clears any in-flight marker state when switching issues", () => {
    const { beginMarkerEdit, init } = useReaderStore.getState();
    beginMarkerEdit({
      kind: "note",
      page_index: 0,
      region: null,
      selection: null,
      body: "draft",
      is_favorite: false,
      tags: [],
    });
    init({
      issueId: "new-issue",
      seriesId: null,
      totalPages: 30,
      initialPage: 0,
      initialDirection: "ltr",
      initialViewMode: "single",
    });
    const s = useReaderStore.getState();
    expect(s.pendingMarker).toBeNull();
    expect(s.editingMarkerId).toBeNull();
    expect(s.markerMode).toBe("idle");
  });
});

describe("reader keybinds — marker defaults", () => {
  // localStorage isn't present in node env; nothing to clean up here.
  afterEach(() => {});

  it("ships `b`, `n`, `h` as default reader bindings", () => {
    expect(KEYBIND_DEFAULTS.bookmarkPage).toBe("b");
    expect(KEYBIND_DEFAULTS.addNote).toBe("n");
    expect(KEYBIND_DEFAULTS.startHighlight).toBe("h");
  });

  it("dispatches the right action for each marker key", () => {
    const bindings = resolveKeybinds(null);
    expect(actionForKey({ key: "b" }, bindings)).toBe("bookmarkPage");
    expect(actionForKey({ key: "n" }, bindings)).toBe("addNote");
    expect(actionForKey({ key: "h" }, bindings)).toBe("startHighlight");
    expect(actionForKey({ key: "B", shiftKey: true }, bindings)).toBeNull();
  });

  it("user override beats the marker defaults", () => {
    const bindings = resolveKeybinds({ bookmarkPage: "Shift+b" });
    expect(actionForKey({ key: "b" }, bindings)).toBeNull();
    expect(actionForKey({ key: "b", shiftKey: true }, bindings)).toBe(
      "bookmarkPage",
    );
  });
});

/** Mirror of the overlay's `dragToRegion` math so resize tolerance can
 *  be locked in without booting the SVG. */
function dragToRegion(
  d: { startX: number; startY: number; currentX: number; currentY: number },
  shape: "rect" | "text" | "image",
): { x: number; y: number; w: number; h: number; shape: string } | null {
  const x = Math.min(d.startX, d.currentX);
  const y = Math.min(d.startY, d.currentY);
  const w = Math.abs(d.currentX - d.startX);
  const h = Math.abs(d.currentY - d.startY);
  if (w < 1 || h < 1) return null;
  return { x, y, w, h, shape };
}

describe("marker overlay — normalized coords", () => {
  it("rejects below-threshold clicks (1% min in either dim)", () => {
    expect(
      dragToRegion(
        { startX: 50, startY: 50, currentX: 50.5, currentY: 51 },
        "rect",
      ),
    ).toBeNull();
  });

  it("orders the drag corners so x/y is the top-left regardless of pull direction", () => {
    const r = dragToRegion(
      { startX: 70, startY: 80, currentX: 30, currentY: 20 },
      "rect",
    );
    expect(r).toEqual({ x: 30, y: 20, w: 40, h: 60, shape: "rect" });
  });

  it("hardcodes the shape tag from the selection mode", () => {
    const r = dragToRegion(
      { startX: 0, startY: 0, currentX: 50, currentY: 33.3333 },
      "text",
    );
    expect(r?.shape).toBe("text");
  });
});
