// @vitest-environment jsdom
/**
 * A reader page that won't load (PageImage).
 *
 * The browser-only half (the retry sits above the reader's tap zones and in
 * view) is covered by tests/e2e/reader-flow.spec.ts.
 */
import { fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { PageImage } from "@/app/[locale]/read/[seriesSlug]/[issueSlug]/PageImage";

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
});

describe("PageImage load failure", () => {
  it("retries once silently, then offers a retry the cover can see", () => {
    const { img, container } = page();
    fireEvent.error(img());
    expect(img().getAttribute("src")).toBe(`${SRC}?r=1`);
    expect(retry()).toBeNull();

    fireEvent.error(img());
    expect(retry()).not.toBeNull();
    // What `firstPageSettled` looks for, so the cover handoff lets go.
    expect(container.querySelector("[data-page-load-error]")).toBe(retry());
  });

  it("the retry re-requests the page", () => {
    const { img } = page();
    fireEvent.error(img());
    fireEvent.error(img());
    fireEvent.click(retry()!);
    expect(img().getAttribute("src")).toBe(`${SRC}?r=2`);
    expect(retry()).toBeNull();
  });

  it("notices an image that failed before hydration", () => {
    // A server-rendered <img> can fail while React isn't listening yet, so
    // no `error` event ever arrives: broken means `complete` with no size.
    vi.spyOn(HTMLImageElement.prototype, "complete", "get").mockReturnValue(
      true,
    );
    const { img } = page();
    expect(img().getAttribute("src")).toBe(`${SRC}?r=1`);
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
