/**
 * CQ-TEST-5 (audit 2026-07): the reader's two every-session behaviors that
 * had zero direct tests — the page-cursor advance/clamp in the store and
 * the RTL arrow-swap in the keymap. An off-by-one or inverted flip here
 * breaks every read session; these pin both.
 */
import { beforeEach, describe, expect, it } from "vitest";

import { pageNavForKey } from "@/lib/reader/keybinds";
import { useReaderStore } from "@/lib/reader/store";

describe("reader store page cursor (advance/clamp)", () => {
  beforeEach(() => {
    useReaderStore.setState({ totalPages: 10, currentPage: 0 });
  });

  it("advances and retreats by one", () => {
    const s = useReaderStore.getState();
    s.nextPage();
    expect(useReaderStore.getState().currentPage).toBe(1);
    useReaderStore.getState().prevPage();
    expect(useReaderStore.getState().currentPage).toBe(0);
  });

  it("clamps at both ends", () => {
    useReaderStore.getState().prevPage();
    expect(useReaderStore.getState().currentPage).toBe(0);
    useReaderStore.getState().setPage(9);
    useReaderStore.getState().nextPage();
    expect(useReaderStore.getState().currentPage).toBe(9);
  });

  it("setPage clamps arbitrary jumps into range", () => {
    useReaderStore.getState().setPage(-5);
    expect(useReaderStore.getState().currentPage).toBe(0);
    useReaderStore.getState().setPage(99);
    expect(useReaderStore.getState().currentPage).toBe(9);
    useReaderStore.getState().setPage(4);
    expect(useReaderStore.getState().currentPage).toBe(4);
  });

  it("no-ops when totalPages is 0 (pre-init)", () => {
    useReaderStore.setState({ totalPages: 0, currentPage: 0 });
    useReaderStore.getState().nextPage();
    expect(useReaderStore.getState().currentPage).toBe(0);
  });
});

describe("pageNavForKey (RTL arrow swap)", () => {
  it("LTR: arrows keep logical meaning", () => {
    expect(pageNavForKey("nextPage", "ArrowRight", "ltr")).toBe("next");
    expect(pageNavForKey("prevPage", "ArrowLeft", "ltr")).toBe("prev");
  });

  it("RTL: visual right goes backwards, left goes forwards", () => {
    expect(pageNavForKey("nextPage", "ArrowRight", "rtl")).toBe("prev");
    expect(pageNavForKey("nextPage", "ArrowLeft", "rtl")).toBe("next");
    expect(pageNavForKey("prevPage", "ArrowLeft", "rtl")).toBe("next");
    expect(pageNavForKey("prevPage", "ArrowRight", "rtl")).toBe("prev");
  });

  it("RTL: non-arrow bindings keep logical meaning", () => {
    expect(pageNavForKey("nextPage", " ", "rtl")).toBe("next");
    expect(pageNavForKey("nextPage", "n", "rtl")).toBe("next");
    expect(pageNavForKey("prevPage", "p", "rtl")).toBe("prev");
  });
});
