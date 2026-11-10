// @vitest-environment jsdom
/**
 * Cover long-press sheet (components/CoverLongPressActions.tsx).
 *
 * Regression: the sheet opens while the finger is still down, and the
 * click the browser synthesizes on lift landed on whichever sheet item
 * was under the finger — a press on a series cover ran "Mark all unread".
 * The sheet now ignores clicks until the opening touch has ended and a
 * new pointer / touch has started on it; keyboard activation is never
 * blocked. jsdom cannot hit-test, so the "lift click" is dispatched on
 * the item directly, which is the worst case.
 */
import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/lib/ui/use-coarse-pointer", () => ({
  useTouchDevice: () => true,
}));

import { useCoverLongPressActions } from "@/components/CoverLongPressActions";

type Finger = { clientX: number; clientY: number };

/** jsdom's TouchEvent rejects plain-object touches; React only reads
 *  `nativeEvent.touches`, so a plain Event carrying them is enough. */
function touch(el: Element, type: string, touches: Finger[] = []) {
  const e = new Event(type, { bubbles: true, cancelable: true });
  Object.defineProperty(e, "touches", { value: touches });
  Object.defineProperty(e, "changedTouches", { value: touches });
  act(() => {
    el.dispatchEvent(e);
  });
  return e;
}

/** The click a finger tap produces: `detail` 1, unlike keyboard (0). */
function tapClick(el: Element) {
  return fireEvent.click(el, { detail: 1 });
}

const onRead = vi.fn();
const onUnread = vi.fn();
/** Whether the last click reaching the link would have navigated. */
let navigated: boolean | null = null;

function Card() {
  const lp = useCoverLongPressActions({
    primary: { label: "Read Saga", onSelect: onRead },
    actions: [{ label: "Mark all unread", onSelect: onUnread }],
    label: "Saga",
  });
  return (
    <>
      <a
        href="/series/saga"
        data-testid="link"
        onClick={(e) => {
          navigated = !e.defaultPrevented;
          // jsdom cannot navigate; keep it from trying.
          e.preventDefault();
        }}
      >
        <div data-testid="cover" {...lp.wrapperProps}>
          cover
        </div>
      </a>
      {lp.sheet}
    </>
  );
}

function longPress(cover: Element) {
  touch(cover, "touchstart", [{ clientX: 20, clientY: 20 }]);
  act(() => {
    vi.advanceTimersByTime(400);
  });
}

beforeEach(() => {
  vi.useFakeTimers();
  onRead.mockReset();
  onUnread.mockReset();
  navigated = null;
});
afterEach(() => {
  act(() => {
    vi.runOnlyPendingTimers();
  });
  vi.useRealTimers();
});

describe("useCoverLongPressActions", () => {
  it("marks the cover so the native long-press callout is suppressed", () => {
    render(<Card />);
    expect(
      screen.getByTestId("cover").hasAttribute("data-cover-long-press"),
    ).toBe(true);
  });

  it("a short tap on the cover still follows the link", () => {
    render(<Card />);
    const cover = screen.getByTestId("cover");
    touch(cover, "touchstart", [{ clientX: 20, clientY: 20 }]);
    act(() => {
      vi.advanceTimersByTime(150);
    });
    touch(cover, "touchend");
    tapClick(cover);
    expect(navigated).toBe(true);
    expect(screen.queryByRole("menuitem")).toBeNull();
  });

  it("the lift that ends the opening press does not activate a sheet item", () => {
    render(<Card />);
    const cover = screen.getByTestId("cover");
    longPress(cover);
    const unread = screen.getByRole("menuitem", { name: "Mark all unread" });
    const read = screen.getByRole("menuitem", { name: "Read Saga" });

    touch(cover, "touchend");
    tapClick(unread);
    tapClick(read);

    expect(onUnread).not.toHaveBeenCalled();
    expect(onRead).not.toHaveBeenCalled();
    // The sheet is still open for the deliberate tap.
    expect(screen.getByRole("menuitem", { name: "Mark all unread" })).toBe(
      unread,
    );
  });

  it("a press on the sheet while the opening finger is still down stays blocked", () => {
    render(<Card />);
    const cover = screen.getByTestId("cover");
    longPress(cover);
    const unread = screen.getByRole("menuitem", { name: "Mark all unread" });
    // Second finger lands on the sheet before the first lifts.
    touch(unread, "touchstart", [{ clientX: 40, clientY: 300 }]);
    tapClick(unread);
    expect(onUnread).not.toHaveBeenCalled();
  });

  it("a second tap on an item, after the lift, activates it", () => {
    render(<Card />);
    const cover = screen.getByTestId("cover");
    longPress(cover);
    const unread = screen.getByRole("menuitem", { name: "Mark all unread" });

    touch(cover, "touchend");
    tapClick(unread);
    expect(onUnread).not.toHaveBeenCalled();

    touch(unread, "touchstart", [{ clientX: 40, clientY: 300 }]);
    touch(unread, "touchend");
    tapClick(unread);
    expect(onUnread).toHaveBeenCalledTimes(1);
  });

  it("the opening lift is seen even when it does not reach the cover", () => {
    render(<Card />);
    const cover = screen.getByTestId("cover");
    longPress(cover);
    const unread = screen.getByRole("menuitem", { name: "Mark all unread" });
    // touchend observed only at the document level.
    touch(document.body, "touchend");
    fireEvent.pointerDown(unread, { pointerType: "touch" });
    tapClick(unread);
    expect(onUnread).toHaveBeenCalledTimes(1);
  });

  it("keyboard activation is never blocked", () => {
    render(<Card />);
    const cover = screen.getByTestId("cover");
    longPress(cover);
    const unread = screen.getByRole("menuitem", { name: "Mark all unread" });
    // Finger still down; a keyboard / switch user activates the item.
    fireEvent.keyDown(unread, { key: "Enter" });
    fireEvent.click(unread, { detail: 0 });
    expect(onUnread).toHaveBeenCalledTimes(1);
  });

  it("the press that opened the sheet does not navigate the cover link", () => {
    render(<Card />);
    const cover = screen.getByTestId("cover");
    longPress(cover);
    touch(cover, "touchend");
    tapClick(cover);
    // The wrapper stops the click before it reaches the link.
    expect(navigated).toBe(null);
  });

  it("cancels the browser menu for a touch long press but not a right-click", () => {
    render(<Card />);
    const cover = screen.getByTestId("cover");
    const contextMenu = (pointerType: string) => {
      const ev = new Event("contextmenu", { bubbles: true, cancelable: true });
      Object.defineProperty(ev, "pointerType", { value: pointerType });
      act(() => {
        cover.dispatchEvent(ev);
      });
      return ev.defaultPrevented;
    };
    expect(contextMenu("mouse")).toBe(false);
    touch(cover, "touchstart", [{ clientX: 20, clientY: 20 }]);
    expect(contextMenu("touch")).toBe(true);
  });
});
