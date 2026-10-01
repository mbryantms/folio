// @vitest-environment jsdom
/**
 * WP-4.2 reader polish — the DOM-bound halves of the behaviours whose pure
 * math is covered in tests/reader/{zoom,detect,use-swipe}.test.ts:
 *
 *  - ctrl+wheel / trackpad-pinch zoom claims the event from the browser
 *    (audit UX-3) and Safari `gesture*` pinch is honoured on desktop;
 *  - the iOS standalone left-edge guard cancels the OS back-swipe and
 *    re-dispatches a tap in the strip as the left tap zone (audit UX-10);
 *  - the progress bar mirrors in RTL (audit UX-5).
 */
import * as React from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render } from "@testing-library/react";

import { useWheelZoom } from "@/lib/reader/use-wheel-zoom";
import { EDGE_GUARD_ATTR, useReaderGestures } from "@/lib/reader/use-swipe";
import { ReadingProgress } from "@/app/[locale]/read/[seriesSlug]/[issueSlug]/ReadingProgress";
import { useReaderStore } from "@/lib/reader/store";
import { ShortcutsSheet } from "@/components/ShortcutsSheet";
import { resolveKeybinds } from "@/lib/reader/keybinds";

function WheelHarness(props: {
  enabled: boolean;
  scale: number;
  onZoomTo: (scale: number, x: number, y: number) => void;
}) {
  const ref = React.useRef<HTMLDivElement>(null);
  useWheelZoom({
    target: ref,
    enabled: props.enabled,
    getScale: () => props.scale,
    onZoomTo: props.onZoomTo,
  });
  return <div ref={ref} data-testid="surface" />;
}

function wheel(el: Element, init: WheelEventInit) {
  const e = new WheelEvent("wheel", {
    bubbles: true,
    cancelable: true,
    ...init,
  });
  el.dispatchEvent(e);
  return e;
}

describe("useWheelZoom", () => {
  it("turns ctrl+wheel into reader zoom and blocks browser zoom", () => {
    const onZoomTo = vi.fn();
    const { getByTestId } = render(
      <WheelHarness enabled scale={1} onZoomTo={onZoomTo} />,
    );
    const e = wheel(getByTestId("surface"), {
      ctrlKey: true,
      deltaY: -10,
      clientX: 120,
      clientY: 80,
    });
    expect(e.defaultPrevented).toBe(true);
    expect(onZoomTo).toHaveBeenCalledTimes(1);
    const [scale, x, y] = onZoomTo.mock.calls[0]!;
    expect(scale).toBeGreaterThan(1);
    expect([x, y]).toEqual([120, 80]);
  });

  it("scales relative to the current zoom", () => {
    const onZoomTo = vi.fn();
    const { getByTestId } = render(
      <WheelHarness enabled scale={2} onZoomTo={onZoomTo} />,
    );
    wheel(getByTestId("surface"), { ctrlKey: true, deltaY: 10 });
    const [scale] = onZoomTo.mock.calls[0]!;
    expect(scale).toBeLessThan(2);
    expect(scale).toBeGreaterThan(1);
  });

  it("leaves a plain wheel (scroll) alone", () => {
    const onZoomTo = vi.fn();
    const { getByTestId } = render(
      <WheelHarness enabled scale={1} onZoomTo={onZoomTo} />,
    );
    const e = wheel(getByTestId("surface"), { deltaY: 40 });
    expect(e.defaultPrevented).toBe(false);
    expect(onZoomTo).not.toHaveBeenCalled();
  });

  it("falls through to the browser when disabled (webtoon / drawing)", () => {
    const onZoomTo = vi.fn();
    const { getByTestId } = render(
      <WheelHarness enabled={false} scale={1} onZoomTo={onZoomTo} />,
    );
    const e = wheel(getByTestId("surface"), { ctrlKey: true, deltaY: -10 });
    expect(e.defaultPrevented).toBe(false);
    expect(onZoomTo).not.toHaveBeenCalled();
  });

  it("handles Safari trackpad pinch (gesture events) on fine pointers", () => {
    const onZoomTo = vi.fn();
    const { getByTestId } = render(
      <WheelHarness enabled scale={1.5} onZoomTo={onZoomTo} />,
    );
    const el = getByTestId("surface");
    const gesture = (type: string, scale: number) => {
      const e = new Event(type, { bubbles: true, cancelable: true });
      Object.assign(e, { scale, clientX: 10, clientY: 20 });
      el.dispatchEvent(e);
      return e;
    };
    expect(gesture("gesturestart", 1).defaultPrevented).toBe(true);
    expect(gesture("gesturechange", 2).defaultPrevented).toBe(true);
    // Cumulative gesture scale × the scale at gesture start.
    expect(onZoomTo).toHaveBeenLastCalledWith(3, 10, 20);
  });
});

function GestureHarness(props: {
  direction: "ltr" | "rtl";
  onNext: () => void;
  onPrev: () => void;
}) {
  const ref = React.useRef<HTMLDivElement>(null);
  useReaderGestures({
    target: ref,
    enabled: true,
    viewMode: "single",
    direction: props.direction,
    onNext: props.onNext,
    onPrev: props.onPrev,
    panActive: false,
    onPanStart: () => {},
    onPan: () => {},
  });
  return (
    <div ref={ref}>
      <div data-testid="page" {...{ [EDGE_GUARD_ATTR]: "" }} />
      <button type="button" data-testid="chrome-button">
        Exit
      </button>
    </div>
  );
}

function touch(
  el: Element,
  type: "touchstart" | "touchend",
  x: number,
  timeStamp = 0,
) {
  const e = new Event(type, { bubbles: true, cancelable: true });
  const point = { identifier: 1, clientX: x, clientY: 300 };
  Object.defineProperty(e, "touches", {
    value: type === "touchstart" ? [point] : [],
  });
  Object.defineProperty(e, "changedTouches", { value: [point] });
  Object.defineProperty(e, "timeStamp", { value: timeStamp });
  el.dispatchEvent(e);
  return e;
}

describe("iOS standalone left-edge guard", () => {
  afterEach(() => {
    delete (window.navigator as { standalone?: boolean }).standalone;
  });

  function setStandalone(v: boolean) {
    Object.defineProperty(window.navigator, "standalone", {
      value: v,
      configurable: true,
    });
  }

  it("cancels an edge touch on the page and turns a tap into the left zone", () => {
    setStandalone(true);
    const onNext = vi.fn();
    const onPrev = vi.fn();
    const { getByTestId } = render(
      <GestureHarness direction="ltr" onNext={onNext} onPrev={onPrev} />,
    );
    const page = getByTestId("page");
    expect(touch(page, "touchstart", 8, 0).defaultPrevented).toBe(true);
    touch(page, "touchend", 9, 120);
    expect(onPrev).toHaveBeenCalledTimes(1);
    expect(onNext).not.toHaveBeenCalled();
  });

  it("maps the edge tap to next page in RTL", () => {
    setStandalone(true);
    const onNext = vi.fn();
    const onPrev = vi.fn();
    const { getByTestId } = render(
      <GestureHarness direction="rtl" onNext={onNext} onPrev={onPrev} />,
    );
    const page = getByTestId("page");
    touch(page, "touchstart", 5, 0);
    touch(page, "touchend", 5, 100);
    expect(onNext).toHaveBeenCalledTimes(1);
  });

  it("leaves touches away from the edge, and chrome buttons, alone", () => {
    setStandalone(true);
    const { getByTestId } = render(
      <GestureHarness direction="ltr" onNext={vi.fn()} onPrev={vi.fn()} />,
    );
    expect(touch(getByTestId("page"), "touchstart", 200).defaultPrevented).toBe(
      false,
    );
    expect(
      touch(getByTestId("chrome-button"), "touchstart", 8).defaultPrevented,
    ).toBe(false);
  });

  it("is inert outside iOS standalone", () => {
    setStandalone(false);
    const onPrev = vi.fn();
    const { getByTestId } = render(
      <GestureHarness direction="ltr" onNext={vi.fn()} onPrev={onPrev} />,
    );
    const page = getByTestId("page");
    expect(touch(page, "touchstart", 8, 0).defaultPrevented).toBe(false);
    touch(page, "touchend", 8, 100);
    expect(onPrev).not.toHaveBeenCalled();
  });
});

describe("ReadingProgress direction", () => {
  beforeEach(() => {
    useReaderStore.setState({ direction: "ltr" });
  });

  it("fills from the left in LTR", () => {
    const { getByRole } = render(<ReadingProgress current={3} total={10} />);
    const bar = getByRole("progressbar");
    expect(bar.dataset.direction).toBe("ltr");
    const fill = bar.firstElementChild as HTMLElement;
    expect(fill.className).toContain("origin-left");
    expect(fill.className).not.toContain("ml-auto");
  });

  it("mirrors (fills from the right) in RTL, following the store", () => {
    useReaderStore.setState({ direction: "rtl" });
    const { getByRole } = render(<ReadingProgress current={3} total={10} />);
    const bar = getByRole("progressbar");
    expect(bar.dataset.direction).toBe("rtl");
    const fill = bar.firstElementChild as HTMLElement;
    expect(fill.className).toContain("ml-auto");
    expect(fill.className).toContain("origin-right");
    // Width (and the reported value) are direction-independent.
    expect(fill.style.width).toBe("40%");
    expect(bar.getAttribute("aria-valuenow")).toBe("40");
  });

  it("an explicit direction prop overrides the store", () => {
    useReaderStore.setState({ direction: "ltr" });
    const { getByRole } = render(
      <ReadingProgress current={1} total={10} direction="rtl" />,
    );
    expect(getByRole("progressbar").dataset.direction).toBe("rtl");
  });
});

describe("shortcuts sheet lists pointer zoom", () => {
  it("documents ctrl+scroll / trackpad pinch and double-click zoom", async () => {
    const { findAllByText } = render(
      <ShortcutsSheet
        open
        onOpenChange={() => {}}
        bindings={resolveKeybinds(null)}
        initialSection="reader"
      />,
    );
    expect(
      (await findAllByText("Zoom at the pointer (or pinch a trackpad)")).length,
    ).toBeGreaterThan(0);
    expect(
      (await findAllByText("Toggle 2× zoom at the pointer")).length,
    ).toBeGreaterThan(0);
  });
});
