/**
 * Global vitest setup. Runs before every test file, in BOTH environments —
 * everything here must be a no-op when `window` is absent (node env).
 *
 * jsdom lacks a few browser APIs the component tree touches on render:
 *  - ResizeObserver: @floating-ui (Radix popover / dropdown positioning)
 *  - matchMedia: coarse-pointer + reduced-motion probes
 *  - Element.scrollIntoView / pointer capture: Radix menu keyboard nav
 * Stubs are inert; tests assert on DOM state, not on layout.
 */
import { afterEach } from "vitest";

if (typeof window !== "undefined") {
  class ResizeObserverStub {
    observe() {}
    unobserve() {}
    disconnect() {}
  }
  if (!("ResizeObserver" in window)) {
    Object.defineProperty(window, "ResizeObserver", {
      value: ResizeObserverStub,
      configurable: true,
      writable: true,
    });
  }
  if (!window.matchMedia) {
    window.matchMedia = (query: string) =>
      ({
        matches: false,
        media: query,
        onchange: null,
        addListener: () => {},
        removeListener: () => {},
        addEventListener: () => {},
        removeEventListener: () => {},
        dispatchEvent: () => false,
      }) as MediaQueryList;
  }
  const proto = window.Element.prototype as unknown as Record<string, unknown>;
  for (const name of [
    "scrollIntoView",
    "hasPointerCapture",
    "setPointerCapture",
    "releasePointerCapture",
  ]) {
    if (typeof proto[name] !== "function") proto[name] = () => false;
  }

  // Unmount rendered trees between tests. @testing-library/react only
  // auto-registers this when `afterEach` is a global (globals: false here).
  afterEach(async () => {
    const { cleanup } = await import("@testing-library/react");
    cleanup();
  });
}
