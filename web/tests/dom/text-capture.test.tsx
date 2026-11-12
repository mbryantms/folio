// @vitest-environment jsdom
/**
 * Text capture in the marker overlay ("Highlight + capture text", `x`),
 * asserted at the network seam: the REQUEST BODIES the reader sends.
 *
 * The page in these tests is 1988 x 3056 on the server, while its `<img>`
 * loaded a 960 px variant from its `srcSet` and reports a density-corrected
 * natural size of 960 x 1476. The server crops the original decode, so
 * every pixel rectangle must be computed against 1988 x 3056 (D1).
 */
import * as React from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type * as Queries from "@/lib/api/queries";

const { toast, apiFetch, savedMarkers } = vi.hoisted(() => ({
  savedMarkers: { items: [] as object[] },
  toast: Object.assign(vi.fn(), {
    error: vi.fn(),
    success: vi.fn(),
    loading: vi.fn(() => "toast-id"),
    dismiss: vi.fn(),
    message: vi.fn(),
    info: vi.fn(),
  }),
  apiFetch: vi.fn(),
}));
vi.mock("sonner", () => ({ toast }));
vi.mock("@/lib/api/auth-refresh", () => ({
  apiFetch,
  getCsrfToken: () => "csrf",
}));
vi.mock("@/lib/api/queries", async (importOriginal) => ({
  ...(await importOriginal<typeof Queries>()),
  useIssueMarkers: () => ({ data: { items: savedMarkers.items } }),
}));

import { MarkerOverlay } from "@/app/[locale]/read/[seriesSlug]/[issueSlug]/MarkerOverlay";
import { PageTextPanel } from "@/app/[locale]/read/[seriesSlug]/[issueSlug]/PageTextPanel";
import { usePageTextPanel } from "@/lib/reader/page-text";
import { useReaderStore } from "@/lib/reader/store";

const SERVER = { width: 1988, height: 3056 };
const BOX = { x: 40, y: 40, w: 20, h: 20, confidence: 0.9, class: 0 };

function json(
  status: number,
  body: unknown,
  headers: Record<string, string> = {},
) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json", ...headers },
  });
}

type OcrBody = {
  page: number;
  region: { x: number; y: number; w: number; h: number };
  detect?: boolean;
};

function ocrBodies(): OcrBody[] {
  return apiFetch.mock.calls
    .filter(([path]) => String(path).endsWith("/ocr"))
    .map(([, init]) => JSON.parse((init as RequestInit).body as string));
}

/** The `<img>` as the reader renders it: a `srcSet` variant, laid out at
 *  100 x 100 CSS px at the origin, reporting the variant's natural size. */
function variantImg(): HTMLImageElement {
  const img = document.createElement("img");
  img.setAttribute("srcset", "/p?w=1080 1080w, /p 1988w");
  Object.defineProperty(img, "naturalWidth", { value: 960 });
  Object.defineProperty(img, "naturalHeight", { value: 1476 });
  Object.defineProperty(img, "complete", { value: true });
  img.getBoundingClientRect = () =>
    ({
      left: 0,
      top: 0,
      width: 100,
      height: 100,
      right: 100,
      bottom: 100,
    }) as DOMRect;
  return img;
}

let ocrResponse: () => Response | Promise<Response>;
let regions: object[];
let regionsResponse: () => Response | Promise<Response>;

function renderOverlay(img = variantImg()) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const imgRef = { current: img };
  const utils = render(
    <QueryClientProvider client={qc}>
      <div style={{ position: "relative" }}>
        <MarkerOverlay
          issueId="issue-1"
          pageIndex={0}
          imgRef={imgRef}
          serverPageSize={SERVER}
        />
      </div>
    </QueryClientProvider>,
  );
  const svg = utils.container.querySelector("svg")!;
  return { ...utils, svg, qc };
}

function pointer(
  svg: Element,
  type: "down" | "move" | "up",
  x: number,
  y: number,
) {
  const init = { clientX: x, clientY: y, pointerId: 1, button: 0 };
  if (type === "down") fireEvent.pointerDown(svg, init);
  else if (type === "move") fireEvent.pointerMove(svg, init);
  else fireEvent.pointerUp(svg, init);
}

beforeEach(() => {
  ocrResponse = () => json(200, { text: "HELLO", confidence: 0.9 });
  regions = [BOX];
  regionsResponse = () =>
    json(200, { page_w: SERVER.width, page_h: SERVER.height, regions });
  apiFetch.mockImplementation(async (path: string) => {
    if (path.includes("/text-regions")) return regionsResponse();
    if (path.endsWith("/ocr")) return ocrResponse();
    throw new Error(`unexpected ${path}`);
  });
  useReaderStore.setState({
    currentPage: 0,
    viewMode: "single",
    direction: "ltr",
    markerMode: "select-text",
    markersHidden: false,
    pendingMarker: null,
    editingMarkerId: null,
    chromeVisible: false,
    chromePinned: false,
  } as never);
});
afterEach(() => {
  apiFetch.mockReset();
  vi.clearAllMocks();
});

async function outlinesLoaded(container: HTMLElement, count = 1) {
  await waitFor(() =>
    expect(
      container.querySelectorAll('[aria-label^="Capture text region"]'),
    ).toHaveLength(count),
  );
}

function deferred<T>() {
  let resolve!: (v: T) => void;
  const promise = new Promise<T>((r) => (resolve = r));
  return { promise, resolve };
}

describe("D1: pixel rectangles are in original page pixels", () => {
  it("a tap on a box sends the box against the server's page size", async () => {
    const { svg, container } = renderOverlay();
    await outlinesLoaded(container);
    act(() => pointer(svg, "down", 50, 50));
    act(() => pointer(svg, "up", 50, 50));
    await waitFor(() => expect(ocrBodies()).toHaveLength(1));
    // 40% / 20% of 1988 x 3056 — not of the 960 x 1476 variant.
    expect(ocrBodies()[0]).toEqual({
      page: 0,
      region: { x: 795, y: 1222, w: 398, h: 611 },
    });
    await waitFor(() =>
      expect(useReaderStore.getState().pendingMarker).not.toBeNull(),
    );
  });

  it("a drag sends the dragged rectangle against the server's page size, and the snapped box comes back to the same percent", async () => {
    ocrResponse = () =>
      json(200, {
        text: "SNAPPED",
        confidence: 0.8,
        refined_bbox: { x: 398, y: 611, w: 597, h: 917 },
      });
    const { svg, container } = renderOverlay();
    await outlinesLoaded(container);
    act(() => pointer(svg, "down", 10, 10));
    act(() => pointer(svg, "move", 30, 20));
    act(() => pointer(svg, "up", 30, 20));
    await waitFor(() => expect(ocrBodies()).toHaveLength(1));
    expect(ocrBodies()[0]).toEqual({
      page: 0,
      region: { x: 199, y: 306, w: 398, h: 306 },
      detect: true,
    });
    await waitFor(() =>
      expect(useReaderStore.getState().pendingMarker).not.toBeNull(),
    );
    const region = useReaderStore.getState().pendingMarker!.region!;
    // refined_bbox / 1988 x 3056, in percent.
    expect(region.x).toBeCloseTo((398 / 1988) * 100, 5);
    expect(region.y).toBeCloseTo((611 / 3056) * 100, 5);
    expect(region.w).toBeCloseTo((597 / 1988) * 100, 5);
    expect(region.h).toBeCloseTo((917 / 3056) * 100, 5);
  });
});

describe("D2: tap, keyboard and panel read the same box", () => {
  // A balloon (block box) and one of its lines (line box) inside it.
  const BLOCK = { x: 30, y: 30, w: 40, h: 40, confidence: 0.9, class: 0 };
  const LINE = { x: 35, y: 45, w: 30, h: 8, confidence: 0.9, class: 1 };
  const BLOCK_PX = { x: 596, y: 917, w: 795, h: 1222 };

  beforeEach(() => {
    regions = [BLOCK, LINE];
  });

  it("a tap on a line inside a balloon reads the whole balloon", async () => {
    const { svg, container } = renderOverlay();
    // One outline / proxy: the nested line is not a box of its own.
    await outlinesLoaded(container, 1);
    expect(container.querySelectorAll("svg rect")).toHaveLength(1);
    act(() => pointer(svg, "down", 50, 49));
    act(() => pointer(svg, "up", 50, 49));
    await waitFor(() => expect(ocrBodies()).toHaveLength(1));
    expect(ocrBodies()[0]).toEqual({ page: 0, region: BLOCK_PX });
  });

  it("the keyboard proxy reads the same box", async () => {
    const { container } = renderOverlay();
    await outlinesLoaded(container, 1);
    fireEvent.click(
      container.querySelector('[aria-label="Capture text region 1 of 1"]')!,
    );
    await waitFor(() => expect(ocrBodies()).toHaveLength(1));
    expect(ocrBodies()[0]).toEqual({ page: 0, region: BLOCK_PX });
  });

  it("the page-text panel reads the same box", async () => {
    const qc = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
    usePageTextPanel.setState({ open: true, firstText: {} });
    render(
      <QueryClientProvider client={qc}>
        <PageTextPanel issueId="issue-1" pages={[0]} direction="ltr" />
      </QueryClientProvider>,
    );
    await waitFor(() => expect(ocrBodies()).toHaveLength(1));
    expect(ocrBodies()[0]).toEqual({ page: 0, region: BLOCK_PX });
    usePageTextPanel.setState({ open: false });
  });

  it("a tap on a box the panel already read uses the panel's result", async () => {
    const { svg, container, qc } = renderOverlay();
    await outlinesLoaded(container, 1);
    // The panel's cache entry for this box (same key the panel writes).
    await qc.fetchQuery({
      queryKey: ["ocr", "region-text", "issue-1", 0, "30.00,30.00,40.00,40.00"],
      queryFn: () => ({
        text: "FROM PANEL",
        confidence: 0.8,
        refinedBbox: null,
        lang: "western",
      }),
    });
    act(() => pointer(svg, "down", 50, 49));
    act(() => pointer(svg, "up", 50, 49));
    await waitFor(() =>
      expect(useReaderStore.getState().pendingMarker?.selection?.text).toBe(
        "FROM PANEL",
      ),
    );
    expect(ocrBodies()).toHaveLength(0);
  });
});

describe("D3: a tap that hits nothing stays in capture mode", () => {
  it("a tap outside every box does nothing", async () => {
    const { svg, container } = renderOverlay();
    await outlinesLoaded(container);
    act(() => pointer(svg, "down", 5, 5));
    act(() => pointer(svg, "up", 5, 5));
    await new Promise((r) => setTimeout(r, 20));
    expect(useReaderStore.getState().markerMode).toBe("select-text");
    expect(useReaderStore.getState().pendingMarker).toBeNull();
    expect(ocrBodies()).toHaveLength(0);
  });

  it("a tap before the outlines loaded does nothing", async () => {
    const pending = deferred<Response>();
    regionsResponse = () => pending.promise;
    const { svg } = renderOverlay();
    act(() => pointer(svg, "down", 50, 50));
    act(() => pointer(svg, "up", 50, 50));
    await new Promise((r) => setTimeout(r, 20));
    expect(useReaderStore.getState().markerMode).toBe("select-text");
    expect(useReaderStore.getState().pendingMarker).toBeNull();
  });
});

describe("D4: one capture at a time, and cancel cancels", () => {
  it("a second tap while one capture is reading starts nothing", async () => {
    const pending = deferred<Response>();
    ocrResponse = () => pending.promise;
    const { svg, container } = renderOverlay();
    await outlinesLoaded(container);
    act(() => pointer(svg, "down", 50, 50));
    act(() => pointer(svg, "up", 50, 50));
    await waitFor(() => expect(ocrBodies()).toHaveLength(1));
    act(() => pointer(svg, "down", 50, 50));
    act(() => pointer(svg, "up", 50, 50));
    fireEvent.click(
      container.querySelector('[aria-label="Capture text region 1 of 1"]')!,
    );
    await new Promise((r) => setTimeout(r, 20));
    expect(ocrBodies()).toHaveLength(1);
    pending.resolve(json(200, { text: "ONCE", confidence: 0.9 }));
    await waitFor(() =>
      expect(useReaderStore.getState().pendingMarker?.selection?.text).toBe(
        "ONCE",
      ),
    );
  });

  it("Esc during recognition leaves the mode and the late result opens nothing", async () => {
    const pending = deferred<Response>();
    ocrResponse = () => pending.promise;
    const { svg, container } = renderOverlay();
    await outlinesLoaded(container);
    act(() => pointer(svg, "down", 50, 50));
    act(() => pointer(svg, "up", 50, 50));
    await waitFor(() => expect(ocrBodies()).toHaveLength(1));
    act(() => {
      fireEvent.keyDown(window, { key: "Escape" });
    });
    expect(useReaderStore.getState().markerMode).toBe("idle");
    pending.resolve(json(200, { text: "LATE", confidence: 0.9 }));
    await new Promise((r) => setTimeout(r, 30));
    expect(useReaderStore.getState().pendingMarker).toBeNull();
  });
});

describe("D5 / D12: failures say why", () => {
  it("a failed capture keeps the mode and says the reason, offering Try again when it can help", async () => {
    ocrResponse = () =>
      json(503, { error: { code: "service_unavailable", message: "busy" } });
    const { svg, container } = renderOverlay();
    await outlinesLoaded(container);
    act(() => pointer(svg, "down", 50, 50));
    act(() => pointer(svg, "up", 50, 50));
    await waitFor(() => expect(toast.error).toHaveBeenCalledTimes(1));
    expect(toast.error.mock.calls[0]?.[0]).toBe(
      "Couldn't read the text. The server is busy reading other text. Try again in a moment.",
    );
    expect(toast.error.mock.calls[0]?.[1]).toMatchObject({
      action: { label: "Try again" },
    });
    expect(useReaderStore.getState().markerMode).toBe("select-text");
    expect(useReaderStore.getState().pendingMarker).toBeNull();
  });

  it("a failed detection shows its reason in a live region instead of nothing", async () => {
    regionsResponse = () =>
      json(409, {
        error: { code: "archive_needs_conversion", message: "convert" },
      });
    const { getByRole } = renderOverlay();
    await waitFor(() =>
      expect(getByRole("status").textContent).toContain(
        "has to be converted to CBZ",
      ),
    );
    // Final: no Try again.
    expect(getByRole("status").querySelector("button")).toBeNull();
  });

  it("no boxes found is said, not silent", async () => {
    regions = [];
    const { getByRole } = renderOverlay();
    await waitFor(() =>
      expect(getByRole("status").textContent).toBe(
        "No text found on page 1. Drag around text to capture it.",
      ),
    );
  });
});

describe("D5: the page-text panel", () => {
  function renderPanel() {
    const qc = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
    usePageTextPanel.setState({ open: true, firstText: {} });
    const utils = render(
      <QueryClientProvider client={qc}>
        <PageTextPanel issueId="issue-1" pages={[0]} direction="ltr" />
      </QueryClientProvider>,
    );
    return { ...utils, qc };
  }
  afterEach(() => usePageTextPanel.setState({ open: false }));

  it("retries a 503 on its own (honouring Retry-After) and never caches it as a result", async () => {
    let calls = 0;
    ocrResponse = () => {
      calls += 1;
      return calls === 1
        ? json(
            503,
            { error: { code: "service_unavailable", message: "busy" } },
            { "retry-after": "1" },
          )
        : json(200, { text: "AFTER RETRY", confidence: 0.9 });
    };
    const { findByText, qc } = renderPanel();
    // While the first answer is a 503, the box's entry holds no result.
    await waitFor(() => expect(calls).toBe(1));
    const key = ["ocr", "region-text", "issue-1", 0, "40.00,40.00,20.00,20.00"];
    expect(qc.getQueryData(key)).toBeUndefined();
    expect(await findByText("AFTER RETRY", {}, { timeout: 4000 })).toBeTruthy();
    expect(calls).toBe(2);
  });

  it("a 409 shows its reason and is not retried", async () => {
    ocrResponse = () =>
      json(409, {
        error: { code: "archive_needs_conversion", message: "convert" },
      });
    const { findByRole } = renderPanel();
    const status = await findByRole("status");
    await waitFor(() =>
      expect(status.textContent).toBe(
        "Couldn't read the text on page 1. This issue's file has to be converted to CBZ before its text can be read.",
      ),
    );
    expect(ocrBodies()).toHaveLength(1);
  });

  it("counts failed regions and offers Try again for them", async () => {
    regions = [BOX, { x: 5, y: 80, w: 10, h: 10, confidence: 0.9, class: 0 }];
    let failSecond = true;
    apiFetch.mockImplementation(async (path: string, init?: RequestInit) => {
      if (path.includes("/text-regions")) return regionsResponse();
      const body = JSON.parse(init!.body as string) as {
        region: { y: number };
      };
      if (body.region.y > 2000 && failSecond) {
        return json(429, { error: { code: "rate_limited", message: "slow" } });
      }
      return json(200, { text: `TEXT ${body.region.y}`, confidence: 0.9 });
    });
    const { findByRole, findByText } = renderPanel();
    // 429 retries (1 s, 2 s, 4 s) before the failure is final.
    const retry = await findByRole(
      "button",
      { name: "Try the failed region again" },
      { timeout: 12_000 },
    );
    expect((await findByRole("status")).textContent).toBe(
      "Page 1: 1 text block. 1 of 2 regions couldn't be read. Too many text requests in a short time. Try again in a moment.",
    );
    failSecond = false;
    fireEvent.click(retry);
    expect(await findByText("TEXT 2445")).toBeTruthy();
  }, 20_000);
});

describe("D12: saved highlights read their captured text", () => {
  afterEach(() => {
    savedMarkers.items = [];
  });
  it("labels a saved highlight's proxy with its text and caption", () => {
    savedMarkers.items = [
      {
        id: "m-1",
        page_index: 0,
        kind: "highlight",
        region: { x: 10, y: 10, w: 20, h: 10, shape: "text" },
        selection: { text: "WE ARE\nBORN", ocr_confidence: 0.9 },
        body: "the opening line",
        is_favorite: false,
        tags: [],
      },
    ];
    useReaderStore.setState({ markerMode: "idle" } as never);
    const { getByRole } = renderOverlay();
    expect(
      getByRole("button", {
        name: "Highlight: “WE ARE BORN” — the opening line",
      }),
    ).toBeTruthy();
  });
});

describe("D10: webtoon keeps vertical scrolling in capture mode", () => {
  it("the overlay allows pan-y in webtoon and none in a paged view", async () => {
    const { svg } = renderOverlay();
    expect((svg as SVGElement).style.touchAction).toBe("none");
    act(() => useReaderStore.setState({ viewMode: "webtoon" } as never));
    expect((svg as SVGElement).style.touchAction).toBe("pan-y pinch-zoom");
  });
});
