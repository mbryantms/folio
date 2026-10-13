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

vi.mock("sonner", () => ({
  Toaster: (props: { position?: string }) => {
    positions.push(props.position ?? "");
    return null;
  },
}));

vi.mock("next-themes", () => ({ useTheme: () => ({ theme: "dark" }) }));

import { Toaster } from "@/components/ui/sonner";

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
