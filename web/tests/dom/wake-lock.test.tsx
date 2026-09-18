// @vitest-environment jsdom
import { act, renderHook } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import {
  useReaderWakeLock,
  useWakePreference,
} from "@/lib/reader/use-wake-lock";
afterEach(() => {
  localStorage.clear();
  useWakePreference.setState({ enabled: false });
});
it("releases on exit and reacquires after returning to a visible reader", async () => {
  localStorage.setItem("folio:keep-awake", "true");
  const release = vi.fn().mockResolvedValue(undefined);
  const request = vi
    .fn()
    .mockResolvedValue({ release, addEventListener: vi.fn() });
  Object.defineProperty(navigator, "wakeLock", {
    configurable: true,
    value: { request },
  });
  Object.defineProperty(document, "visibilityState", {
    configurable: true,
    value: "visible",
  });
  const { unmount } = renderHook(() => useReaderWakeLock());
  await act(async () => {});
  expect(request).toHaveBeenCalledWith("screen");
  Object.defineProperty(document, "visibilityState", {
    configurable: true,
    value: "hidden",
  });
  await act(async () => {
    document.dispatchEvent(new Event("visibilitychange"));
  });
  expect(release).toHaveBeenCalledOnce();
  Object.defineProperty(document, "visibilityState", {
    configurable: true,
    value: "visible",
  });
  await act(async () => {
    document.dispatchEvent(new Event("visibilitychange"));
  });
  expect(request).toHaveBeenCalledTimes(2);
  unmount();
  expect(release).toHaveBeenCalledTimes(2);
});
