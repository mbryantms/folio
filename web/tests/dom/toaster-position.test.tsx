// @vitest-environment jsdom
/**
 * Toasts anchor to the bottom-right on desktop but to the top edge on
 * phones, where dialogs are bottom sheets and the tab bar owns the
 * bottom: a bottom-anchored toast sat on exactly the buttons the person
 * was about to tap (iOS report, 2026-10-13).
 */
import { render } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

const positions: string[] = [];
const styles: Record<string, string>[] = [];

vi.mock("sonner", () => ({
  Toaster: (props: { position?: string; style?: Record<string, string> }) => {
    positions.push(props.position ?? "");
    styles.push(props.style ?? {});
    return null;
  },
}));

vi.mock("next-themes", () => ({ useTheme: () => ({ theme: "dark" }) }));

import { Toaster } from "@/components/folio/sonner";

function stubViewport(matches: boolean) {
  Object.defineProperty(window, "matchMedia", {
    configurable: true,
    writable: true,
    value: (query: string) => ({
      matches,
      media: query,
      addEventListener: () => {},
      removeEventListener: () => {},
      addListener: () => {},
      removeListener: () => {},
      onchange: null,
      dispatchEvent: () => false,
    }),
  });
}

afterEach(() => {
  positions.length = 0;
  styles.length = 0;
});

describe("Toaster position", () => {
  it("stays bottom-right on desktop", () => {
    stubViewport(false);
    render(<Toaster />);
    expect(positions.at(-1)).toBe("bottom-right");
  });

  it("moves to the top on phones", () => {
    stubViewport(true);
    render(<Toaster />);
    expect(positions.at(-1)).toBe("top-center");
  });
});

/**
 * The vendored Toaster spreads our props after its own `style`, so the
 * wrapper's style object replaces upstream's token mapping. Keep the
 * popover / border / radius tokens on it, or Sonner paints its own black
 * and white palette with fixed 8px corners (2026-10-30).
 */
describe("Toaster theme tokens", () => {
  it("maps Sonner's palette onto the theme tokens", () => {
    stubViewport(false);
    render(<Toaster />);
    const style = styles.at(-1) ?? {};
    expect(style["--normal-bg"]).toBe("var(--popover)");
    expect(style["--normal-text"]).toBe("var(--popover-foreground)");
    expect(style["--normal-border"]).toBe("var(--border)");
    expect(style["--border-radius"]).toBe("var(--radius)");
    // The safe-area offsets the wrapper exists for are still there.
    expect(style["--offset-bottom"]).toBe("var(--overlay-bottom)");
  });
});
