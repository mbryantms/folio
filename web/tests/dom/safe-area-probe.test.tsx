// @vitest-environment jsdom
import * as React from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
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

beforeEach(() => {
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
  document.documentElement.style.removeProperty("--safe-top");
  standalone.value = true;
});

describe("SafeAreaProbe (jsdom)", () => {
  it("pins --safe-top to 0 when the OS already reserved the status bar", () => {
    setGeometry(834, 1194 - 24);
    render(<SafeAreaProbe />);
    act(() => {
      vi.advanceTimersByTime(100);
    });
    expect(document.documentElement.style.getPropertyValue("--safe-top")).toBe(
      "0px",
    );
  });

  it("keeps the pin when a later resize reports stale full-height geometry (iPadOS 26.1 reader)", async () => {
    setGeometry(834, 1194 - 24);
    render(<SafeAreaProbe />);
    act(() => {
      vi.advanceTimersByTime(100);
    });
    setGeometry(834, 1194);
    await act(async () => {
      window.dispatchEvent(new Event("resize"));
    });
    expect(document.documentElement.style.getPropertyValue("--safe-top")).toBe(
      "0px",
    );
  });

  it("pins late when the first measurement was edge to edge but a later one shows the reserved bar", async () => {
    setGeometry(834, 1194);
    render(<SafeAreaProbe />);
    act(() => {
      vi.advanceTimersByTime(100);
    });
    expect(document.documentElement.style.getPropertyValue("--safe-top")).toBe(
      "",
    );
    setGeometry(834, 1194 - 24);
    await act(async () => {
      window.dispatchEvent(new Event("pageshow"));
      vi.advanceTimersByTime(100);
    });
    expect(document.documentElement.style.getPropertyValue("--safe-top")).toBe(
      "0px",
    );
  });

  it("does nothing in a browser tab", () => {
    standalone.value = false;
    setGeometry(834, 1194 - 24);
    render(<SafeAreaProbe />);
    act(() => {
      vi.advanceTimersByTime(100);
    });
    expect(document.documentElement.style.getPropertyValue("--safe-top")).toBe(
      "",
    );
  });

  it("removes its pin on unmount", () => {
    setGeometry(834, 1194 - 24);
    const { unmount } = render(<SafeAreaProbe />);
    act(() => {
      vi.advanceTimersByTime(100);
    });
    expect(document.documentElement.style.getPropertyValue("--safe-top")).toBe(
      "0px",
    );
    unmount();
    expect(document.documentElement.style.getPropertyValue("--safe-top")).toBe(
      "",
    );
  });
});
