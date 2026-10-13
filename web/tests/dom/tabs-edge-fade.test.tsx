// @vitest-environment jsdom
/**
 * A tab strip that overflows fades the edge that hides more tabs and
 * clears the fade once scrolled to that end. Scrollbars stay hidden: on
 * iOS they only show during a swipe, so a clipped last tab used to read
 * as a layout bug (CBL manage sheet, 2026-10-13).
 */
import { act, render } from "@testing-library/react";
import { afterAll, afterEach, describe, expect, it } from "vitest";

import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";

const box = { scrollWidth: 0, clientWidth: 0, scrollLeft: 0 };
const defs: Array<[string, PropertyDescriptor | undefined]> = [];

function layout(scrollWidth: number, clientWidth: number, scrollLeft = 0) {
  Object.assign(box, { scrollWidth, clientWidth, scrollLeft });
}

for (const key of ["scrollWidth", "clientWidth", "scrollLeft"] as const) {
  defs.push([key, Object.getOwnPropertyDescriptor(HTMLElement.prototype, key)]);
  Object.defineProperty(HTMLElement.prototype, key, {
    configurable: true,
    get() {
      return box[key];
    },
    set(v: number) {
      box[key] = v;
    },
  });
}

afterEach(() => {
  layout(0, 0, 0);
});

function strip() {
  const utils = render(
    <Tabs defaultValue="a">
      <TabsList>
        <TabsTrigger value="a">Reading order</TabsTrigger>
        <TabsTrigger value="b">Resolution</TabsTrigger>
        <TabsTrigger value="c">History</TabsTrigger>
        <TabsTrigger value="d">Settings</TabsTrigger>
      </TabsList>
    </Tabs>,
  );
  return utils.container.querySelector('[role="tablist"]') as HTMLElement;
}

describe("TabsList edge fade", () => {
  it("fades the right edge when tabs overflow, both in the middle, left at the end", () => {
    layout(600, 300, 0);
    const list = strip();
    expect(list.getAttribute("data-fade")).toBe("right");
    expect(list.className).toContain("data-[fade=right]:mask-r-from-");
    act(() => {
      list.scrollLeft = 150;
      list.dispatchEvent(new Event("scroll"));
    });
    expect(list.getAttribute("data-fade")).toBe("both");
    act(() => {
      list.scrollLeft = 300;
      list.dispatchEvent(new Event("scroll"));
    });
    expect(list.getAttribute("data-fade")).toBe("left");
  });

  it("has no fade when every tab fits", () => {
    layout(300, 300, 0);
    const list = strip();
    expect(list.hasAttribute("data-fade")).toBe(false);
    // No scrollbar is ever drawn for the strip.
    expect(list.className).toContain("[scrollbar-width:none]");
  });
});

// Restore the prototype descriptors for other files in this worker.
afterAll(() => {
  for (const [key, d] of defs) {
    if (d) Object.defineProperty(HTMLElement.prototype, key, d);
  }
});
