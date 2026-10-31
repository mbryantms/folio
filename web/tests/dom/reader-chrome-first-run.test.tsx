// @vitest-environment jsdom
/**
 * First-open chrome default (audit N-R3 / AC-2, remediation C5). On a
 * coarse primary pointer the reader opens with its controls shown until
 * the user hides them once; that first explicit hide is persisted and
 * later opens start hidden. Fine pointers keep the hidden default. The
 * idle auto-hide and the double-tap's optimistic toggle are not choices
 * and must not persist anything.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { DOUBLE_TAP_MS } from "@/lib/reader/zoom";
import {
  initialChromeVisible,
  loadChromeDismissed,
  useReaderStore,
} from "@/lib/reader/store";

function pointer(kind: "coarse" | "fine") {
  window.matchMedia = (query: string) =>
    ({
      matches: query === "(pointer: coarse)" && kind === "coarse",
      media: query,
      onchange: null,
      addListener: () => {},
      removeListener: () => {},
      addEventListener: () => {},
      removeEventListener: () => {},
      dispatchEvent: () => false,
    }) as MediaQueryList;
}

function open(issueId = "issue-1") {
  useReaderStore.getState().init({
    issueId,
    seriesId: null,
    totalPages: 10,
    initialPage: 0,
    initialDirection: "ltr",
    initialViewMode: "single",
  });
  return useReaderStore.getState();
}

beforeEach(() => {
  window.localStorage.clear();
  vi.useFakeTimers();
});

afterEach(() => {
  vi.runOnlyPendingTimers();
  vi.useRealTimers();
  window.localStorage.clear();
});

describe("initialChromeVisible", () => {
  it("is shown on a coarse pointer with no stored choice", () => {
    pointer("coarse");
    expect(initialChromeVisible()).toBe(true);
    expect(initialChromeVisible(true)).toBe(true);
  });

  it("stays hidden on a fine pointer", () => {
    pointer("fine");
    expect(initialChromeVisible()).toBe(false);
    expect(initialChromeVisible(false)).toBe(false);
  });
});

describe("reader store — first-open chrome", () => {
  it("opens shown on touch, hidden on desktop", () => {
    pointer("coarse");
    expect(open().chromeVisible).toBe(true);
    pointer("fine");
    expect(open().chromeVisible).toBe(false);
  });

  it("the first tap-to-hide sticks: later opens start hidden", () => {
    pointer("coarse");
    expect(open().chromeVisible).toBe(true);
    useReaderStore.getState().toggleChrome();
    expect(useReaderStore.getState().chromeVisible).toBe(false);
    // Not recorded until it has outlived the double-tap window.
    expect(loadChromeDismissed()).toBe(false);
    vi.advanceTimersByTime(DOUBLE_TAP_MS + 100);
    expect(loadChromeDismissed()).toBe(true);
    expect(open("issue-2").chromeVisible).toBe(false);
    // Showing it again later does not undo the choice.
    useReaderStore.getState().toggleChrome();
    vi.advanceTimersByTime(DOUBLE_TAP_MS + 100);
    expect(open("issue-3").chromeVisible).toBe(false);
  });

  it("Esc (dismissChrome) records at once", () => {
    pointer("coarse");
    open();
    useReaderStore.getState().dismissChrome();
    expect(useReaderStore.getState().chromeVisible).toBe(false);
    expect(useReaderStore.getState().pageStripVisible).toBe(false);
    expect(loadChromeDismissed()).toBe(true);
    expect(open("issue-2").chromeVisible).toBe(false);
  });

  it("the idle auto-hide is not a choice: setChromeVisible(false) persists nothing", () => {
    pointer("coarse");
    open();
    useReaderStore.getState().setChromeVisible(false);
    vi.advanceTimersByTime(DOUBLE_TAP_MS + 100);
    expect(loadChromeDismissed()).toBe(false);
    expect(open("issue-2").chromeVisible).toBe(true);
  });

  it("a double-tap's hide-then-show inside the window is not a choice", () => {
    pointer("coarse");
    open();
    const { toggleChrome } = useReaderStore.getState();
    toggleChrome(); // first tap: optimistic hide
    vi.advanceTimersByTime(DOUBLE_TAP_MS - 50);
    toggleChrome(); // second tap: TapZones undoes the toggle and zooms
    vi.advanceTimersByTime(DOUBLE_TAP_MS + 100);
    expect(loadChromeDismissed()).toBe(false);
    expect(open("issue-2").chromeVisible).toBe(true);
  });

  it("a double-tap's show-then-hide (chrome was down) is not a choice either", () => {
    pointer("coarse");
    open();
    useReaderStore.getState().setChromeVisible(false); // auto-hidden
    const { toggleChrome } = useReaderStore.getState();
    toggleChrome(); // first tap: optimistic show
    vi.advanceTimersByTime(DOUBLE_TAP_MS - 50);
    toggleChrome(); // second tap: TapZones undoes the toggle and zooms
    vi.advanceTimersByTime(DOUBLE_TAP_MS + 100);
    expect(useReaderStore.getState().chromeVisible).toBe(false);
    expect(loadChromeDismissed()).toBe(false);
    expect(open("issue-2").chromeVisible).toBe(true);
  });

  it("Esc with only the page strip up (chrome already hidden) records nothing", () => {
    pointer("coarse");
    open();
    useReaderStore.getState().setChromeVisible(false); // auto-hidden
    useReaderStore.getState().togglePageStrip(); // `m`
    useReaderStore.getState().dismissChrome(); // Esc closes the strip
    expect(useReaderStore.getState().pageStripVisible).toBe(false);
    expect(loadChromeDismissed()).toBe(false);
    expect(open("issue-2").chromeVisible).toBe(true);
  });

  it("a stored choice also keeps a fine pointer hidden", () => {
    pointer("fine");
    open();
    useReaderStore.getState().toggleChrome(); // `t`: show
    useReaderStore.getState().dismissChrome(); // Esc: hide, a choice
    expect(loadChromeDismissed()).toBe(true);
    expect(open("issue-2").chromeVisible).toBe(false);
  });
});
