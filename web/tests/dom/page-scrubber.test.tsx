// @vitest-environment jsdom
/**
 * Reader page scrubber (`PageScrubber.tsx`): a keyboard `role="slider"`
 * whose keys jump at once, and a pointer drag that previews the target
 * page ("p / N" + strip thumbnail) and commits on release. Geometry and
 * key mapping are covered in tests/reader/scrub.test.ts.
 */
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

import { PageScrubber } from "@/app/[locale]/read/[seriesSlug]/[issueSlug]/PageScrubber";
import { useReaderStore } from "@/lib/reader/store";
import { SWIPE_IGNORE_ATTR } from "@/lib/reader/use-swipe";

function setup(direction: "ltr" | "rtl" = "ltr") {
  useReaderStore.setState({ totalPages: 11, currentPage: 3 });
  const utils = render(
    <PageScrubber
      issueId="iss"
      totalPages={11}
      currentPage={useReaderStore.getState().currentPage}
      direction={direction}
      urlVersion={null}
    />,
  );
  const slider = screen.getByRole("slider", { name: "Page" });
  slider.getBoundingClientRect = () =>
    ({
      left: 0,
      width: 1000,
      top: 0,
      height: 20,
      right: 1000,
      bottom: 20,
    }) as DOMRect;
  return { ...utils, slider };
}

beforeEach(() => {
  useReaderStore.setState({ totalPages: 11, currentPage: 3 });
});
afterEach(cleanup);

describe("PageScrubber", () => {
  it("is a named slider over 1..N that opts out of the swipe layer", () => {
    const { slider } = setup();
    expect(slider.getAttribute("aria-valuemin")).toBe("1");
    expect(slider.getAttribute("aria-valuemax")).toBe("11");
    expect(slider.getAttribute("aria-valuenow")).toBe("4");
    expect(slider.getAttribute("aria-valuetext")).toBe("Page 4 of 11");
    expect(slider.tabIndex).toBe(0);
    expect(slider.hasAttribute(SWIPE_IGNORE_ATTR)).toBe(true);
  });

  it("jumps at once on arrow / Home / End keys", () => {
    const { slider } = setup();
    fireEvent.keyDown(slider, { key: "ArrowRight" });
    expect(useReaderStore.getState().currentPage).toBe(4);
    fireEvent.keyDown(slider, { key: "End" });
    expect(useReaderStore.getState().currentPage).toBe(10);
  });

  it("flips horizontal arrows in RTL", () => {
    const { slider } = setup("rtl");
    fireEvent.keyDown(slider, { key: "ArrowLeft" });
    expect(useReaderStore.getState().currentPage).toBe(4);
  });

  it("previews while dragging and commits on release", () => {
    const { slider } = setup();
    fireEvent.pointerDown(slider, {
      pointerId: 1,
      clientX: 100,
      button: 0,
      pointerType: "mouse",
    });
    fireEvent.pointerMove(slider, {
      pointerId: 1,
      clientX: 700,
      pointerType: "mouse",
    });
    expect(screen.getByTestId("page-scrubber-preview").textContent).toBe(
      "8 / 11",
    );
    // Not yet: the page changes on release.
    expect(useReaderStore.getState().currentPage).toBe(3);
    fireEvent.pointerUp(slider, {
      pointerId: 1,
      clientX: 700,
      pointerType: "mouse",
    });
    expect(useReaderStore.getState().currentPage).toBe(7);
  });

  it("does not jump on a touch tap that never moved", () => {
    const { slider } = setup();
    fireEvent.pointerDown(slider, {
      pointerId: 2,
      clientX: 900,
      pointerType: "touch",
    });
    fireEvent.pointerUp(slider, {
      pointerId: 2,
      clientX: 900,
      pointerType: "touch",
    });
    expect(useReaderStore.getState().currentPage).toBe(3);
    expect(screen.queryByTestId("page-scrubber-preview")).toBeNull();
  });

  it("shows the mouse-hover preview and hides it on leave", () => {
    const { slider } = setup();
    fireEvent.pointerMove(slider, {
      pointerId: 3,
      clientX: 0,
      pointerType: "mouse",
    });
    expect(screen.getByTestId("page-scrubber-preview").textContent).toBe(
      "1 / 11",
    );
    fireEvent.pointerLeave(slider, { pointerId: 3, pointerType: "mouse" });
    expect(screen.queryByTestId("page-scrubber-preview")).toBeNull();
  });
});
