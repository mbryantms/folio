// @vitest-environment jsdom
/**
 * The reader's exit target (audit U-02): the page this tab was on before
 * the reader, kept across next / previous issue (which replace the reader
 * entry), absent on a cold load.
 */
import { renderHook } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type * as BackFlight from "@/lib/ui/back-flight";

type Flight = typeof BackFlight;
let flight: Flight;

beforeEach(async () => {
  // The history stack is module state: start each case with a fresh tab.
  vi.resetModules();
  flight = await import("@/lib/ui/back-flight");
});

const pop = () => window.dispatchEvent(new PopStateEvent("popstate"));

describe("reader exit origin", () => {
  it("a push into the reader, then replaces, keeps the origin", () => {
    flight.noteRoute("/");
    flight.noteRoute("/series/a");
    flight.noteRoute("/read/a/1");
    expect(flight.readerExitOrigin()).toEqual({
      pathname: "/series/a",
      steps: 1,
    });

    flight.noteReplace("/read/a/2?cbl=list-1");
    flight.noteRoute("/read/a/2");
    flight.noteReplace("/read/a/3");
    flight.noteRoute("/read/a/3");
    expect(flight.readerExitOrigin()).toEqual({
      pathname: "/series/a",
      steps: 1,
    });

    // Back from the reader lands on the origin, with the reader forward.
    pop();
    flight.noteRoute("/series/a");
    expect(flight.readerExitOrigin()).toBeNull();
  });

  it("a cold load has no origin", () => {
    flight.noteRoute("/read/a/1");
    expect(flight.readerExitOrigin()).toBeNull();
    flight.noteReplace("/read/a/2");
    flight.noteRoute("/read/a/2");
    expect(flight.readerExitOrigin()).toBeNull();
  });

  it("nested reader entries step back past all of them", () => {
    flight.noteRoute("/search");
    flight.noteRoute("/read/a/1");
    // Pushed, not replaced (a link that opens another reader).
    flight.noteRoute("/read/b/7");
    expect(flight.readerExitOrigin()).toEqual({
      pathname: "/search",
      steps: 2,
    });
  });

  it("sign-in is never the origin", () => {
    flight.noteRoute("/sign-in");
    flight.noteRoute("/read/a/1");
    expect(flight.readerExitOrigin()).toBeNull();
  });

  it("an announced replace that never lands is forgotten", () => {
    flight.noteRoute("/series/a");
    flight.noteReplace("/read/a/9");
    // A different navigation happens instead: it is a push as usual.
    flight.noteRoute("/read/a/1");
    expect(flight.readerExitOrigin()).toEqual({
      pathname: "/series/a",
      steps: 1,
    });
  });
});

describe("exitReader", () => {
  const router = () => ({ back: vi.fn(), replace: vi.fn() });

  it("goes back one entry to the origin", () => {
    flight.noteRoute("/");
    flight.noteRoute("/read/a/1");
    const r = router();
    flight.exitReader(r, "/series/a/issues/1");
    expect(r.back).toHaveBeenCalledTimes(1);
    expect(r.replace).not.toHaveBeenCalled();
  });

  it("goes back through nested reader entries", () => {
    const go = vi.spyOn(window.history, "go").mockImplementation(() => {});
    flight.noteRoute("/");
    flight.noteRoute("/read/a/1");
    flight.noteRoute("/read/a/2");
    const r = router();
    flight.exitReader(r, "/series/a/issues/2");
    expect(go).toHaveBeenCalledWith(-2);
    expect(r.back).not.toHaveBeenCalled();
    go.mockRestore();
  });

  it("on a cold load replaces the reader with the fallback, tracked", () => {
    flight.noteRoute("/read/a/1");
    const r = router();
    flight.exitReader(r, "/series/a/issues/1");
    expect(r.replace).toHaveBeenCalledWith("/series/a/issues/1");
    // The tracker swaps the reader entry for the issue page, so that page
    // offers no in-app Back (there is nothing behind it).
    flight.noteRoute("/series/a/issues/1");
    expect(renderHook(() => flight.useCanGoBack()).result.current).toBe(false);
  });
});
