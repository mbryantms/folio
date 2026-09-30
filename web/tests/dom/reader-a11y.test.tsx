// @vitest-environment jsdom
/**
 * Reader accessibility pass (WP-4.8, audit AC-2 / AC-3):
 *
 *  - MarkerOverlay renders focusable, reading-ordered proxies for detected
 *    text regions in text-capture mode, and activating one runs the same
 *    tap-to-OCR capture a pointer tap does.
 *  - PageTextPanel lists the page's OCR text in reading order, exposes its
 *    phase through a status region, tags manga text `lang="ja"`, and keeps
 *    Esc from reaching the reader keymap.
 *  - ReaderSkipLinks reveal the hidden chrome / open the panel.
 *
 * API hooks and the OCR helper are mocked at the module seam.
 */
import * as React from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type * as Queries from "@/lib/api/queries";
import type * as Mutations from "@/lib/api/mutations";
import type { TextRegionsView } from "@/lib/api/types";

const { toast, ocr, regionsState } = vi.hoisted(() => ({
  toast: Object.assign(vi.fn(), {
    error: vi.fn(),
    success: vi.fn(),
    loading: vi.fn(() => "toast-id"),
    dismiss: vi.fn(),
    message: vi.fn(),
    info: vi.fn(),
  }),
  ocr: vi.fn(),
  regionsState: {
    current: {} as {
      data?: TextRegionsView;
      isLoading?: boolean;
      isError?: boolean;
      isSuccess?: boolean;
    },
  },
}));
vi.mock("sonner", () => ({ toast }));
vi.mock("@/lib/api/queries", async (importOriginal) => ({
  ...(await importOriginal<typeof Queries>()),
  useIssueMarkers: () => ({ data: { items: [] } }),
  useIssuePageTextRegions: (_issue: string, _page: number, enabled: boolean) =>
    enabled
      ? { refetch: vi.fn(), ...regionsState.current }
      : { data: undefined, isLoading: false, isError: false, refetch: vi.fn() },
}));
vi.mock("@/lib/api/mutations", async (importOriginal) => ({
  ...(await importOriginal<typeof Mutations>()),
  useCreateMarker: () => ({ mutate: vi.fn(), isPending: false }),
  useDeleteMarker: () => ({ mutate: vi.fn(), isPending: false }),
}));
vi.mock(
  "@/app/[locale]/read/[seriesSlug]/[issueSlug]/marker-selection",
  () => ({ ocrCroppedRegion: ocr, sha256CroppedRegion: vi.fn() }),
);

import { MarkerOverlay } from "@/app/[locale]/read/[seriesSlug]/[issueSlug]/MarkerOverlay";
import { PageTextPanel } from "@/app/[locale]/read/[seriesSlug]/[issueSlug]/PageTextPanel";
import { ReaderSkipLinks } from "@/app/[locale]/read/[seriesSlug]/[issueSlug]/ReaderSkipLinks";
import { useReaderStore } from "@/lib/reader/store";
import { usePageTextPanel } from "@/lib/reader/page-text";

// Two bubbles on one row (given right-then-left), one on the next row, and
// a line box nested in the first bubble that must not become its own stop.
const REGIONS: TextRegionsView = {
  page_w: 1000,
  page_h: 1500,
  regions: [
    { x: 60, y: 10, w: 30, h: 10, confidence: 0.9, class: 0 },
    { x: 10, y: 11, w: 30, h: 10, confidence: 0.9, class: 0 },
    { x: 62, y: 12, w: 20, h: 4, confidence: 0.9, class: 1 },
    { x: 20, y: 60, w: 40, h: 10, confidence: 0.9, class: 0 },
  ],
};

function withQueryClient(node: React.ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={qc}>{node}</QueryClientProvider>;
}

beforeEach(() => {
  regionsState.current = {
    data: REGIONS,
    isLoading: false,
    isError: false,
    isSuccess: true,
  };
  useReaderStore.setState({
    currentPage: 0,
    viewMode: "single",
    direction: "ltr",
    markerMode: "idle",
    markersHidden: false,
    pendingMarker: null,
    editingMarkerId: null,
    chromeVisible: false,
    chromePinned: false,
  } as never);
  usePageTextPanel.setState({ open: false });
});
afterEach(() => {
  ocr.mockReset();
  vi.clearAllMocks();
});

describe("MarkerOverlay text-region keyboard proxies", () => {
  function renderOverlay() {
    const img = document.createElement("img");
    const imgRef = { current: img };
    return render(
      withQueryClient(
        <div style={{ position: "relative" }}>
          <MarkerOverlay
            issueId="issue-1"
            pageIndex={0}
            imgRef={imgRef}
            naturalSize={{ width: 1000, height: 1500 }}
          />
        </div>,
      ),
    );
  }

  it("renders nothing focusable for regions outside text-capture mode", () => {
    renderOverlay();
    expect(
      screen.queryByRole("button", { name: /Capture text region/ }),
    ).toBeNull();
  });

  it("exposes one focusable proxy per bubble, in reading order", () => {
    useReaderStore.setState({ markerMode: "select-text" } as never);
    renderOverlay();
    const group = screen.getByRole("group", {
      name: "Detected text on page 1",
    });
    const proxies = within(group).getAllByRole("button");
    // Nested line box collapsed → 3 stops, not 4.
    expect(proxies.map((b) => b.getAttribute("aria-label"))).toEqual([
      "Capture text region 1 of 3",
      "Capture text region 2 of 3",
      "Capture text region 3 of 3",
    ]);
    // Reading order: left bubble of the top row first.
    expect(proxies[0]!.style.left).toBe("10%");
    expect(proxies[1]!.style.left).toBe("60%");
    expect(proxies[2]!.style.top).toBe("60%");
    for (const p of proxies) {
      expect(p.tabIndex).toBe(0);
      // Pointer input stays with the SVG drag surface.
      expect(p.className).toContain("pointer-events-none");
    }
    expect(group.textContent).toContain("3 text regions found");
  });

  it("follows RTL reading order", () => {
    useReaderStore.setState({
      markerMode: "select-text",
      direction: "rtl",
    } as never);
    renderOverlay();
    const proxies = screen.getAllByRole("button", {
      name: /Capture text region/,
    });
    expect(proxies[0]!.style.left).toBe("60%");
    expect(proxies[1]!.style.left).toBe("10%");
  });

  it("activating a proxy OCRs that bubble and opens the editor", async () => {
    ocr.mockResolvedValue({
      text: "HELLO THERE",
      confidence: 0.9,
      refinedBbox: null,
      lang: "western",
    });
    useReaderStore.setState({ markerMode: "select-text" } as never);
    renderOverlay();
    const first = screen.getByRole("button", {
      name: "Capture text region 1 of 3",
    });
    first.focus();
    expect(document.activeElement).toBe(first);
    // Keyboard activation of a <button> dispatches click.
    fireEvent.click(first);
    await waitFor(() =>
      expect(useReaderStore.getState().pendingMarker).not.toBeNull(),
    );
    expect(ocr).toHaveBeenCalledOnce();
    const [input, opts] = ocr.mock.calls[0]!;
    expect(input).toMatchObject({
      issueId: "issue-1",
      pageIndex: 0,
      region: { x: 10, y: 11, w: 30, h: 10, shape: "text" },
    });
    expect(opts).toEqual({ detect: false });
    expect(useReaderStore.getState().pendingMarker?.selection).toEqual({
      text: "HELLO THERE",
      ocr_confidence: 0.9,
    });
  });
});

describe("PageTextPanel", () => {
  function renderPanel(
    pages: number[] = [0],
    direction: "ltr" | "rtl" = "ltr",
  ) {
    return render(
      withQueryClient(
        <PageTextPanel issueId="issue-1" pages={pages} direction={direction} />,
      ),
    );
  }

  it("does no OCR work while closed", () => {
    renderPanel();
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(ocr).not.toHaveBeenCalled();
  });

  it("lists the page text in reading order once open", async () => {
    const texts: Record<number, string> = { 10: "FIRST", 60: "SECOND" };
    ocr.mockImplementation(
      async (input: { region: { x: number; y: number } }) =>
        input.region.y === 60
          ? { text: "THIRD", confidence: 0.8, refinedBbox: null, lang: "manga" }
          : {
              text: texts[input.region.x]!,
              confidence: 0.8,
              refinedBbox: null,
              lang: "western",
            },
    );
    renderPanel();
    act(() => usePageTextPanel.getState().setOpen(true));
    const dialog = await screen.findByRole("dialog", { name: "Page text" });
    const list = await within(dialog).findByRole("list", {
      name: "Text on page 1",
    });
    await waitFor(() =>
      expect(within(list).getAllByRole("listitem")).toHaveLength(3),
    );
    const items = within(list).getAllByRole("listitem");
    expect(items.map((li) => li.textContent)).toEqual([
      "FIRST",
      "SECOND",
      "THIRD",
    ]);
    expect(items[0]!.getAttribute("lang")).toBe("en");
    expect(items[2]!.getAttribute("lang")).toBe("ja");
    expect(within(dialog).getByRole("status").textContent).toBe(
      "Page 1: 3 text blocks.",
    );
    // Nested line box is never OCR'd on its own.
    expect(ocr).toHaveBeenCalledTimes(3);
  });

  it("announces when the page has no detected text", async () => {
    regionsState.current = {
      data: { page_w: 10, page_h: 10, regions: [] },
      isSuccess: true,
    };
    renderPanel();
    act(() => usePageTextPanel.getState().setOpen(true));
    const dialog = await screen.findByRole("dialog", { name: "Page text" });
    expect(within(dialog).getByRole("status").textContent).toBe(
      "No text detected on page 1.",
    );
    expect(ocr).not.toHaveBeenCalled();
  });

  it("offers a retry when detection fails", async () => {
    regionsState.current = { data: undefined, isError: true };
    renderPanel();
    act(() => usePageTextPanel.getState().setOpen(true));
    const dialog = await screen.findByRole("dialog", { name: "Page text" });
    expect(within(dialog).getByRole("status").textContent).toBe(
      "Couldn't detect text on page 1.",
    );
    expect(
      within(dialog).getByRole("button", { name: "Try again" }),
    ).toBeTruthy();
  });

  it("labels each page of a double-page spread", async () => {
    regionsState.current = {
      data: { page_w: 10, page_h: 10, regions: [] },
      isSuccess: true,
    };
    renderPanel([2, 3]);
    act(() => usePageTextPanel.getState().setOpen(true));
    const dialog = await screen.findByRole("dialog", { name: "Page text" });
    expect(
      within(dialog).getByRole("heading", { name: "Page 3" }),
    ).toBeTruthy();
    expect(
      within(dialog).getByRole("heading", { name: "Page 4" }),
    ).toBeTruthy();
  });

  it("closes on Escape without the key reaching the reader keymap", async () => {
    regionsState.current = {
      data: { page_w: 10, page_h: 10, regions: [] },
      isSuccess: true,
    };
    const readerKeymap = vi.fn();
    window.addEventListener("keydown", readerKeymap);
    try {
      renderPanel();
      act(() => usePageTextPanel.getState().setOpen(true));
      const dialog = await screen.findByRole("dialog", { name: "Page text" });
      fireEvent.keyDown(dialog, { key: "Escape" });
      await waitFor(() => expect(usePageTextPanel.getState().open).toBe(false));
      expect(readerKeymap).not.toHaveBeenCalled();
    } finally {
      window.removeEventListener("keydown", readerKeymap);
    }
  });
});

describe("ReaderSkipLinks", () => {
  it("reveals the chrome and opens the page-text panel", () => {
    render(<ReaderSkipLinks toggleChromeKey="t" pageTextKey="r" />);
    const nav = screen.getByRole("navigation", { name: "Reader shortcuts" });
    const [controls, text] = within(nav).getAllByRole("button");
    expect(controls!.textContent).toBe("Show reader controls (T)");
    expect(controls!.getAttribute("aria-keyshortcuts")).toBe("t");
    expect(text!.textContent).toBe("Show page text (R)");

    fireEvent.click(controls!);
    expect(useReaderStore.getState().chromeVisible).toBe(true);
    fireEvent.click(text!);
    expect(usePageTextPanel.getState().open).toBe(true);
  });

  it("names a rebound shortcut", () => {
    render(<ReaderSkipLinks toggleChromeKey="c" pageTextKey="Shift+R" />);
    expect(
      screen.getByRole("button", { name: /^Show reader controls \(C\)/ }),
    ).toBeTruthy();
  });
});
