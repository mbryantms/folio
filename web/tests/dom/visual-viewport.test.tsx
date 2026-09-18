// @vitest-environment jsdom
import { act, render } from "@testing-library/react";
import { expect, it } from "vitest";
import { VisualViewportSync } from "@/components/VisualViewportSync";
it("clears keyboard space without moving sheets during pinch magnification", () => {
  const viewport = Object.assign(new EventTarget(), {
    height: 500,
    offsetTop: 0,
    scale: 1,
  });
  Object.defineProperty(window, "visualViewport", {
    configurable: true,
    value: viewport,
  });
  Object.defineProperty(window, "innerHeight", {
    configurable: true,
    value: 800,
  });
  const { unmount } = render(<VisualViewportSync />);
  expect(
    document.documentElement.style.getPropertyValue("--keyboard-inset"),
  ).toBe("300px");
  viewport.scale = 2;
  viewport.height = 250;
  act(() => {
    viewport.dispatchEvent(new Event("resize"));
  });
  expect(
    document.documentElement.style.getPropertyValue("--keyboard-inset"),
  ).toBe("300px");
  viewport.scale = 1;
  viewport.height = 800;
  act(() => {
    viewport.dispatchEvent(new Event("resize"));
  });
  expect(
    document.documentElement.style.getPropertyValue("--keyboard-inset"),
  ).toBe("0px");
  unmount();
  Object.defineProperty(window, "visualViewport", {
    configurable: true,
    value: undefined,
  });
});
