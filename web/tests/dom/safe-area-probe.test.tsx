// @vitest-environment jsdom
import * as React from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { act, render } from "@testing-library/react";

const standalone = vi.hoisted(() => ({ value: true }));
vi.mock("@/lib/use-pull-to-refresh", () => ({
  isStandaloneDisplay: () => standalone.value,
}));

import { SafeAreaProbe } from "@/components/SafeAreaProbe";

function setGeometry(
  innerWidth: number,
  innerHeight: number,
  sw = 834,
  sh = 1194,
) {
  Object.defineProperty(window, "innerWidth", {
    configurable: true,
    value: innerWidth,
  });
  Object.defineProperty(window, "innerHeight", {
    configurable: true,
    value: innerHeight,
  });
  Object.defineProperty(window, "screen", {
    configurable: true,
    value: { width: sw, height: sh },
  });
}

afterEach(() => {
  document.documentElement.style.removeProperty("--safe-top");
  standalone.value = true;
});

describe("SafeAreaProbe (jsdom)", () => {
  it("pins --safe-top to 0 when the OS already reserved the status bar, and tracks resizes", async () => {
    setGeometry(834, 1194 - 24);
    render(<SafeAreaProbe />);
    expect(document.documentElement.style.getPropertyValue("--safe-top")).toBe(
      "0px",
    );

    // Viewport grows back to full height (edge to edge again) → hand back to env().
    setGeometry(834, 1194);
    await act(async () => {
      window.dispatchEvent(new Event("resize"));
    });
    expect(document.documentElement.style.getPropertyValue("--safe-top")).toBe(
      "",
    );
  });

  it("does nothing in a browser tab", () => {
    standalone.value = false;
    setGeometry(834, 1194 - 24);
    render(<SafeAreaProbe />);
    expect(document.documentElement.style.getPropertyValue("--safe-top")).toBe(
      "",
    );
  });

  it("removes its pin on unmount", () => {
    setGeometry(834, 1194 - 24);
    const { unmount } = render(<SafeAreaProbe />);
    expect(document.documentElement.style.getPropertyValue("--safe-top")).toBe(
      "0px",
    );
    unmount();
    expect(document.documentElement.style.getPropertyValue("--safe-top")).toBe(
      "",
    );
  });
});
