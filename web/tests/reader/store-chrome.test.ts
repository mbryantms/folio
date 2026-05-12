/**
 * M1 — chrome auto-hide / pin state on the reader store. The store is the
 * source of truth for the chrome's animated visibility; the DOM rendering
 * itself is exercised in (forthcoming) DOM tests once a jsdom env exists.
 */
import { beforeEach, describe, expect, it } from "vitest";
import { useReaderStore } from "@/lib/reader/store";

function reset() {
  useReaderStore.setState({
    issueId: "",
    seriesId: null,
    currentPage: 0,
    totalPages: 0,
    chromeVisible: true,
    chromeAutoHide: true,
    chromePinned: false,
    pageStripVisible: false,
    brightness: 1,
    sepia: 0,
    coverSolo: true,
  });
}

describe("reader store — chrome controls", () => {
  beforeEach(() => {
    reset();
  });

  it("defaults chromeAutoHide on, chromePinned off, chrome visible", () => {
    const s = useReaderStore.getState();
    expect(s.chromeVisible).toBe(true);
    expect(s.chromeAutoHide).toBe(true);
    expect(s.chromePinned).toBe(false);
  });

  it("toggleChrome flips chromeVisible without touching auto-hide / pinned", () => {
    const { toggleChrome } = useReaderStore.getState();
    toggleChrome();
    let s = useReaderStore.getState();
    expect(s.chromeVisible).toBe(false);
    expect(s.chromeAutoHide).toBe(true);
    expect(s.chromePinned).toBe(false);
    toggleChrome();
    s = useReaderStore.getState();
    expect(s.chromeVisible).toBe(true);
  });

  it("toggleChrome carries pageStripVisible along (open/fade together)", () => {
    const { toggleChrome } = useReaderStore.getState();
    expect(useReaderStore.getState().pageStripVisible).toBe(false);
    toggleChrome();
    let s = useReaderStore.getState();
    expect(s.chromeVisible).toBe(false);
    expect(s.pageStripVisible).toBe(false);
    toggleChrome();
    s = useReaderStore.getState();
    expect(s.chromeVisible).toBe(true);
    expect(s.pageStripVisible).toBe(true);
  });

  it("setChromeVisible mirrors the value into pageStripVisible", () => {
    const { setChromeVisible } = useReaderStore.getState();
    setChromeVisible(false);
    let s = useReaderStore.getState();
    expect(s.chromeVisible).toBe(false);
    expect(s.pageStripVisible).toBe(false);
    setChromeVisible(true);
    s = useReaderStore.getState();
    expect(s.chromeVisible).toBe(true);
    expect(s.pageStripVisible).toBe(true);
  });

  it("togglePageStrip flips only pageStripVisible (the m-keybind path)", () => {
    useReaderStore.setState({ chromeVisible: false, pageStripVisible: false });
    useReaderStore.getState().togglePageStrip();
    const s = useReaderStore.getState();
    expect(s.pageStripVisible).toBe(true);
    expect(s.chromeVisible).toBe(false);
  });

  it("setChromeAutoHide updates only the auto-hide flag", () => {
    const { setChromeAutoHide } = useReaderStore.getState();
    setChromeAutoHide(false);
    let s = useReaderStore.getState();
    expect(s.chromeAutoHide).toBe(false);
    expect(s.chromeVisible).toBe(true);
    setChromeAutoHide(true);
    s = useReaderStore.getState();
    expect(s.chromeAutoHide).toBe(true);
  });

  it("setChromePinned updates only the pinned flag", () => {
    const { setChromePinned } = useReaderStore.getState();
    setChromePinned(true);
    let s = useReaderStore.getState();
    expect(s.chromePinned).toBe(true);
    expect(s.chromeVisible).toBe(true);
    setChromePinned(false);
    s = useReaderStore.getState();
    expect(s.chromePinned).toBe(false);
  });

  it("init preserves user's chromeAutoHide preference", () => {
    const { setChromeAutoHide, init } = useReaderStore.getState();
    setChromeAutoHide(false);
    init({
      issueId: "issue-1",
      seriesId: null,
      totalPages: 10,
      initialPage: 0,
      initialDirection: "ltr",
      initialViewMode: "single",
    });
    const s = useReaderStore.getState();
    expect(s.chromeAutoHide).toBe(false);
    // chromeVisible always resets to true on a fresh issue mount.
    expect(s.chromeVisible).toBe(true);
    // Pinned never carries across mounts — interactive UIs re-pin themselves.
    expect(s.chromePinned).toBe(false);
  });

  it("init restores chromeVisible to true even if previously hidden", () => {
    useReaderStore.setState({ chromeVisible: false });
    useReaderStore.getState().init({
      issueId: "issue-1",
      seriesId: null,
      totalPages: 10,
      initialPage: 0,
      initialDirection: "ltr",
      initialViewMode: "single",
    });
    expect(useReaderStore.getState().chromeVisible).toBe(true);
  });

  it("setBrightness clamps to [0.5, 1.5]", () => {
    const { setBrightness } = useReaderStore.getState();
    setBrightness(0.1);
    expect(useReaderStore.getState().brightness).toBe(0.5);
    setBrightness(2.5);
    expect(useReaderStore.getState().brightness).toBe(1.5);
    setBrightness(1.1);
    expect(useReaderStore.getState().brightness).toBeCloseTo(1.1, 5);
  });

  it("setSepia clamps to [0, 1]", () => {
    const { setSepia } = useReaderStore.getState();
    setSepia(-0.5);
    expect(useReaderStore.getState().sepia).toBe(0);
    setSepia(2);
    expect(useReaderStore.getState().sepia).toBe(1);
    setSepia(0.4);
    expect(useReaderStore.getState().sepia).toBeCloseTo(0.4, 5);
  });

  it("init carries vision adjustments forward across mounts", () => {
    const { setBrightness, setSepia, init } = useReaderStore.getState();
    setBrightness(0.7);
    setSepia(0.3);
    init({
      issueId: "issue-2",
      seriesId: null,
      totalPages: 5,
      initialPage: 0,
      initialDirection: "ltr",
      initialViewMode: "single",
    });
    const s = useReaderStore.getState();
    expect(s.brightness).toBeCloseTo(0.7, 5);
    expect(s.sepia).toBeCloseTo(0.3, 5);
  });
});
