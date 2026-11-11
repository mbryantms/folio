// @vitest-environment jsdom
/**
 * Thumbnails and covers re-request a failed load after a short back-off
 * (a 429 from the thumbnail bucket looks like any other `<img>` error),
 * then give up and report the failure.
 */
import { act, fireEvent, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { Cover } from "@/components/Cover";
import { IMAGE_RETRY_DELAYS_MS, RetryingImg } from "@/components/RetryingImg";

const SRC = "/issues/i1/pages/3/thumb?variant=strip";

beforeEach(() => {
  vi.useFakeTimers();
});
afterEach(() => {
  vi.useRealTimers();
});

const img = (c: HTMLElement) => c.querySelector("img")!;

describe("RetryingImg", () => {
  it("re-requests the same URL with a fresh element after each back-off, then reports the failure", () => {
    const onError = vi.fn();
    const { container } = render(
      <RetryingImg src={SRC} alt="" onError={onError} />,
    );
    for (const delay of IMAGE_RETRY_DELAYS_MS) {
      const before = img(container);
      fireEvent.error(before);
      expect(onError).not.toHaveBeenCalled();
      // Nothing re-requests before the back-off ends.
      act(() => vi.advanceTimersByTime(delay - 1));
      expect(img(container)).toBe(before);
      act(() => vi.advanceTimersByTime(1));
      // A new element, same URL: the browser fetches again, caches keyed
      // by URL are untouched.
      expect(img(container)).not.toBe(before);
      expect(img(container).getAttribute("src")).toBe(SRC);
    }
    fireEvent.error(img(container));
    expect(onError).toHaveBeenCalledTimes(1);
    act(() => vi.advanceTimersByTime(60_000));
    expect(onError).toHaveBeenCalledTimes(1);
  });

  it("a load after a retry needs nothing more", () => {
    const onError = vi.fn();
    const onLoad = vi.fn();
    const { container } = render(
      <RetryingImg src={SRC} alt="" onError={onError} onLoad={onLoad} />,
    );
    fireEvent.error(img(container));
    act(() => vi.advanceTimersByTime(IMAGE_RETRY_DELAYS_MS[0]));
    fireEvent.load(img(container));
    expect(onLoad).toHaveBeenCalledTimes(1);
    expect(onError).not.toHaveBeenCalled();
  });

  it("a new src starts with the full retry budget", () => {
    const onError = vi.fn();
    const { container, rerender } = render(
      <RetryingImg src={SRC} alt="" onError={onError} />,
    );
    for (const delay of IMAGE_RETRY_DELAYS_MS) {
      fireEvent.error(img(container));
      act(() => vi.advanceTimersByTime(delay));
    }
    rerender(
      <RetryingImg src="/issues/i2/pages/0/thumb" alt="" onError={onError} />,
    );
    fireEvent.error(img(container));
    expect(onError).not.toHaveBeenCalled();
  });

  it("unmounting cancels a pending retry", () => {
    const { container, unmount } = render(<RetryingImg src={SRC} alt="" />);
    fireEvent.error(img(container));
    unmount();
    expect(() => act(() => vi.advanceTimersByTime(5_000))).not.toThrow();
  });
});

describe("Cover", () => {
  it("keeps the cover hidden while it retries and settles once retries are spent", () => {
    const { container } = render(
      <Cover src="/issues/i1/pages/0/thumb" alt="c" />,
    );
    fireEvent.error(img(container));
    expect(img(container).className).toContain("opacity-0");
    for (const delay of IMAGE_RETRY_DELAYS_MS) {
      act(() => vi.advanceTimersByTime(delay));
      fireEvent.error(img(container));
    }
    expect(img(container).className).toContain("opacity-100");
  });
});
