// @vitest-environment jsdom
import { act, renderHook } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
const state = vi.hoisted(() => ({
  copy: vi.fn(),
  success: vi.fn(),
  error: vi.fn(),
}));
vi.mock("@/components/ui/copy-button", () => ({
  useCopyToClipboard: () => ({ copy: state.copy }),
}));
vi.mock("sonner", () => ({
  toast: { success: state.success, error: state.error },
}));
import { useShareLink } from "@/lib/ui/use-share-link";
beforeEach(() => {
  vi.clearAllMocks();
  state.copy.mockResolvedValue(true);
});
it("supports native desktop sharing and retains an explicit Copy link action", async () => {
  const share = vi.fn().mockResolvedValue(undefined);
  Object.defineProperty(navigator, "share", {
    configurable: true,
    value: share,
  });
  const { result } = renderHook(() => useShareLink());
  expect(result.current.canShare).toBe(true);
  await act(async () => {
    await result.current.shareOrCopy("/bookmarks", "Bookmarks");
  });
  expect(share).toHaveBeenCalled();
  expect(state.copy).not.toHaveBeenCalled();
  await act(async () => {
    await result.current.copyLink("/bookmarks");
  });
  expect(state.copy).toHaveBeenCalled();
});
it("cancellation is quiet but actual share failure offers a copied link", async () => {
  const share = vi
    .fn()
    .mockRejectedValueOnce(new DOMException("cancelled", "AbortError"))
    .mockRejectedValueOnce(new DOMException("denied", "NotAllowedError"));
  Object.defineProperty(navigator, "share", {
    configurable: true,
    value: share,
  });
  const { result } = renderHook(() => useShareLink());
  await act(async () => {
    await result.current.shareOrCopy("/bookmarks");
  });
  expect(state.copy).not.toHaveBeenCalled();
  await act(async () => {
    await result.current.shareOrCopy("/bookmarks");
  });
  expect(state.copy).toHaveBeenCalledOnce();
});
