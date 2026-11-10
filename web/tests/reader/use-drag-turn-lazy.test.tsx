// @vitest-environment jsdom
/**
 * Drag-to-turn loads after the first paint (`use-drag-turn.ts` +
 * `drag-turn-engine.ts`). Until the engine chunk has resolved, or when it
 * fails to load, the hook must leave swipes to the plain threshold turn
 * (`onTurnDragEnd` → false) and never throw; once it resolves, drags are
 * followed and turn with momentum as before.
 */
import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const { loader } = vi.hoisted(() => ({ loader: vi.fn() }));
vi.mock("@/lib/reader/drag-turn-loader", () => ({
  loadDragTurnEngine: loader,
}));

import * as engineModule from "@/lib/reader/drag-turn-engine";
import { useDragTurn } from "@/lib/reader/use-drag-turn";

function deferred<T>() {
  let resolve!: (v: T) => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

function setup(overrides: { enabled?: boolean } = {}) {
  const track = document.createElement("div");
  const parent = document.createElement("div");
  parent.appendChild(track);
  Object.defineProperty(parent, "clientWidth", { value: 400 });
  const onNext = vi.fn();
  const onPrev = vi.fn();
  const warm = vi.fn();
  const hook = renderHook(() =>
    useDragTurn({
      enabled: overrides.enabled ?? true,
      position: 0,
      canNext: true,
      canPrev: false,
      direction: "ltr",
      onNext,
      onPrev,
      trackRef: { current: track },
      warm,
    }),
  );
  return { ...hook, track, onNext, onPrev, warm };
}

beforeEach(() => {
  vi.useFakeTimers();
  loader.mockReset();
});
afterEach(() => {
  vi.useRealTimers();
});

describe("useDragTurn before its engine has loaded", () => {
  it("fetches the engine (and warms the peek) only once the browser is idle", async () => {
    const gate = deferred<typeof engineModule>();
    loader.mockReturnValue(gate.promise);
    const { warm } = setup();
    expect(loader).not.toHaveBeenCalled();
    await act(async () => {
      vi.advanceTimersByTime(300);
    });
    expect(loader).toHaveBeenCalledTimes(1);
    expect(warm).toHaveBeenCalledTimes(1);
  });

  it("while pending, a swipe is left to the plain turn", async () => {
    loader.mockReturnValue(new Promise(() => {}));
    const { result, track } = setup();
    act(() => result.current.onTurnDrag(-150));
    expect(result.current.dragging).toBe(false);
    expect(track.style.transform).toBe("");
    // false = use-swipe falls back to swipeAction → onNext / onPrev.
    expect(result.current.onTurnDragEnd(-150, false)).toBe(false);
    expect(result.current.consumeSkip()).toBe(false);
    // The first drag asked for the engine without waiting for idle.
    expect(loader).toHaveBeenCalledTimes(1);
  });

  it("does not fetch anything while drag-to-turn is off", async () => {
    loader.mockReturnValue(new Promise(() => {}));
    const { result } = setup({ enabled: false });
    await act(async () => {
      vi.advanceTimersByTime(5000);
    });
    act(() => result.current.onTurnDrag(-150));
    expect(loader).not.toHaveBeenCalled();
    expect(result.current.onTurnDragEnd(-150, false)).toBe(false);
  });
});

describe("useDragTurn once its engine has loaded", () => {
  it("follows the drag and turns with momentum", async () => {
    loader.mockResolvedValue(engineModule);
    const { result, track, onNext } = setup();
    await act(async () => {
      vi.advanceTimersByTime(300);
    });
    act(() => result.current.onTurnDrag(-200));
    expect(result.current.dragging).toBe(true);
    expect(track.style.transform).toMatch(/^translate3d\(-200px/);
    let handled = false;
    act(() => {
      handled = result.current.onTurnDragEnd(-200, false);
    });
    expect(handled).toBe(true);
    await act(async () => {
      vi.advanceTimersByTime(400);
    });
    expect(onNext).toHaveBeenCalledTimes(1);
    // The turn was animated by the drag: the slide transition skips it.
    expect(result.current.consumeSkip()).toBe(true);
  });
});

describe("useDragTurn when its engine fails to load", () => {
  it("keeps swipes on the plain turn and retries on a later drag", async () => {
    const unhandled = vi.fn();
    process.on("unhandledRejection", unhandled);
    try {
      loader.mockRejectedValue(new TypeError("Failed to fetch"));
      const { result } = setup();
      await act(async () => {
        vi.advanceTimersByTime(300);
      });
      expect(loader).toHaveBeenCalledTimes(1);
      act(() => result.current.onTurnDrag(-150));
      expect(result.current.dragging).toBe(false);
      expect(result.current.onTurnDragEnd(-150, false)).toBe(false);
      await act(async () => {});
      // The drag above asked again.
      expect(loader).toHaveBeenCalledTimes(2);

      // A later load that succeeds brings the motion back.
      loader.mockResolvedValue(engineModule);
      act(() => result.current.onTurnDrag(-150));
      await act(async () => {});
      act(() => result.current.onTurnDrag(-160));
      expect(result.current.dragging).toBe(true);
      expect(unhandled).not.toHaveBeenCalled();
    } finally {
      process.off("unhandledRejection", unhandled);
    }
  });
});
