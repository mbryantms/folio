// @vitest-environment jsdom
/**
 * A reader page that won't load (PageImage).
 *
 * The browser-only half (the retry sits above the reader's tap zones and in
 * view) is covered by tests/e2e/reader-flow.spec.ts.
 */
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

// Every retry first renews a lapsing session; resolved at once here.
const ensureFreshSession = vi.fn(async () => {});
vi.mock("@/lib/api/session-refresh", () => ({
  ensureFreshSession: () => ensureFreshSession(),
}));

import {
  PageImage,
  retryTopPx,
} from "@/app/[locale]/read/[seriesSlug]/[issueSlug]/PageImage";

const SRC = "/issues/i1/pages/5";

function page() {
  const view = render(
    <PageImage src={SRC} alt="Page 6" fitClass="w-full" thumbSrc="/thumb" />,
  );
  const img = () =>
    view.container.querySelector<HTMLImageElement>('img[alt="Page 6"]')!;
  return { ...view, img };
}
const retry = () =>
  screen.queryByRole("button", { name: /Couldn.t load this page/ });

afterEach(() => {
  vi.restoreAllMocks();
  ensureFreshSession.mockClear();
});

describe("PageImage load failure", () => {
  it("retries once silently, then offers a retry the cover can see", async () => {
    const { img, container } = page();
    fireEvent.error(img());
    await waitFor(() => expect(img().getAttribute("src")).toBe(`${SRC}?r=1`));
    expect(retry()).toBeNull();

    fireEvent.error(img());
    expect(retry()).not.toBeNull();
    // What `firstPageSettled` looks for, so the cover handoff lets go.
    expect(container.querySelector("[data-page-load-error]")).toBe(retry());
  });

  it("the retry re-requests the page", async () => {
    const { img } = page();
    fireEvent.error(img());
    await waitFor(() => expect(img().getAttribute("src")).toBe(`${SRC}?r=1`));
    fireEvent.error(img());
    fireEvent.click(retry()!);
    await waitFor(() => expect(img().getAttribute("src")).toBe(`${SRC}?r=2`));
    expect(retry()).toBeNull();
  });

  it("renews the session before the automatic retry and before Tap to retry", async () => {
    let release!: () => void;
    ensureFreshSession.mockImplementationOnce(
      () => new Promise<void>((r) => (release = r)),
    );
    const { img } = page();
    fireEvent.error(img());
    expect(ensureFreshSession).toHaveBeenCalledTimes(1);
    // The re-request waits for the renewal.
    expect(img().getAttribute("src")).toBe(SRC);
    release();
    await waitFor(() => expect(img().getAttribute("src")).toBe(`${SRC}?r=1`));

    fireEvent.error(img());
    fireEvent.click(retry()!);
    expect(ensureFreshSession).toHaveBeenCalledTimes(2);
    await waitFor(() => expect(img().getAttribute("src")).toBe(`${SRC}?r=2`));
  });

  it("notices an image that failed before hydration", async () => {
    // A server-rendered <img> can fail while React isn't listening yet, so
    // no `error` event ever arrives: broken means `complete` with no size.
    vi.spyOn(HTMLImageElement.prototype, "complete", "get").mockReturnValue(
      true,
    );
    const { img } = page();
    await waitFor(() => expect(img().getAttribute("src")).toBe(`${SRC}?r=1`));
  });

  it("an image still loading is left alone", () => {
    vi.spyOn(HTMLImageElement.prototype, "complete", "get").mockReturnValue(
      false,
    );
    const { img } = page();
    expect(img().getAttribute("src")).toBe(SRC);
    expect(retry()).toBeNull();
  });
});

describe("PageImage retry stays in view", () => {
  it("sits in the middle of the part of the page on screen", () => {
    // A 3000 px page scrolled 2000 px down in an 800 px window: the
    // visible band is box 2000..2800, so the retry centres at 2400.
    expect(retryTopPx({ top: -2000, bottom: 1000 }, 800)).toBe(2400);
    // Page shorter than the window: the middle of the page.
    expect(retryTopPx({ top: 100, bottom: 500 }, 800)).toBe(200);
    // Zoomed 2x: screen px halve into box px.
    expect(retryTopPx({ top: -2000, bottom: 1000 }, 800, 2)).toBe(1200);
  });

  it("pins the shown retry to the viewport, following scroll", () => {
    vi.spyOn(window, "requestAnimationFrame").mockImplementation((cb) => {
      cb(0);
      return 1;
    });
    let top = 0;
    vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(
      () => ({ top, bottom: top + 3000, height: 3000 }) as DOMRect,
    );
    Object.defineProperty(window, "innerHeight", {
      configurable: true,
      value: 800,
    });
    const { img } = page();
    fireEvent.error(img());
    fireEvent.error(img());
    expect(retry()!.style.top).toBe("400px");

    top = -2000;
    fireEvent.scroll(window);
    expect(retry()!.style.top).toBe("2400px");
  });
});
