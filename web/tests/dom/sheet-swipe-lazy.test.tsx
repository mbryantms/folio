// @vitest-environment jsdom
/**
 * `SheetSwipe` (root layout) fetches the drag-to-dismiss code after
 * hydration instead of shipping it in every page's first load. When the
 * chunk arrives it installs the gesture (and uninstalls it on unmount);
 * when the chunk fails it renders nothing and never reaches an error
 * boundary — sheets still close by button, Escape and overlay.
 */
import * as React from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { render, waitFor } from "@testing-library/react";

class Boundary extends React.Component<
  { children: React.ReactNode },
  { failed: boolean }
> {
  state = { failed: false };
  static getDerivedStateFromError() {
    return { failed: true };
  }
  render() {
    return this.state.failed ? <p>boundary hit</p> : this.props.children;
  }
}

afterEach(() => {
  vi.doUnmock("@/lib/ui/sheet-swipe");
  vi.resetModules();
});

describe("SheetSwipe (lazy gesture chunk)", () => {
  it("installs the gesture once its chunk resolves and removes it on unmount", async () => {
    const uninstall = vi.fn();
    const installSheetSwipe = vi.fn(() => uninstall);
    vi.doMock("@/lib/ui/sheet-swipe", () => ({ installSheetSwipe }));
    const { SheetSwipe } = await import("@/components/SheetSwipe");

    const { unmount, container } = render(<SheetSwipe />);
    expect(container.textContent).toBe("");
    await waitFor(() => expect(installSheetSwipe).toHaveBeenCalledTimes(1));
    expect(uninstall).not.toHaveBeenCalled();
    unmount();
    expect(uninstall).toHaveBeenCalledTimes(1);
  });

  it("does not install when unmounted before the chunk arrives", async () => {
    const installSheetSwipe = vi.fn(() => () => {});
    vi.doMock("@/lib/ui/sheet-swipe", () => ({ installSheetSwipe }));
    const { SheetSwipe } = await import("@/components/SheetSwipe");

    const { unmount } = render(<SheetSwipe />);
    unmount();
    await new Promise((r) => setTimeout(r, 20));
    expect(installSheetSwipe).not.toHaveBeenCalled();
  });

  it("renders nothing and stays out of the error boundary when its chunk fails", async () => {
    vi.doMock("@/lib/ui/sheet-swipe", () => {
      throw new TypeError("Failed to fetch dynamically imported module");
    });
    const { SheetSwipe } = await import("@/components/SheetSwipe");

    const { container } = render(
      <Boundary>
        <SheetSwipe />
      </Boundary>,
    );
    await new Promise((r) => setTimeout(r, 20));
    expect(container.textContent).toBe("");
  });

  it("wires a sheet that was already open before the chunk arrived", async () => {
    const { SheetSwipe } = await import("@/components/SheetSwipe");
    const sheet = document.createElement("div");
    sheet.dataset.slot = "sheet-content";
    sheet.dataset.side = "bottom";
    document.body.append(sheet);
    const spy = vi.spyOn(sheet, "addEventListener");

    const { unmount } = render(<SheetSwipe />);
    await waitFor(() =>
      expect(spy).toHaveBeenCalledWith("pointerdown", expect.any(Function)),
    );
    unmount();
    sheet.remove();
  });
});
