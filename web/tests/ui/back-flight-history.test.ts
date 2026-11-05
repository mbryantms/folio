// @vitest-environment jsdom
import { renderHook } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { noteRoute, useCanGoBack } from "@/lib/ui/back-flight";

const canGoBack = () => renderHook(() => useCanGoBack()).result.current;

describe("in-app back history", () => {
  it("tracks pushes, pops and forward moves", () => {
    noteRoute("/");
    expect(canGoBack()).toBe(false);

    noteRoute("/series/a");
    noteRoute("/series/a/issues/1");
    expect(canGoBack()).toBe(true);

    // Same path again (search-param replace): no new entry.
    noteRoute("/series/a/issues/1");

    // Back to the series page, then back home: nothing left to go back to.
    window.dispatchEvent(new PopStateEvent("popstate"));
    noteRoute("/series/a");
    expect(canGoBack()).toBe(true);
    window.dispatchEvent(new PopStateEvent("popstate"));
    noteRoute("/");
    expect(canGoBack()).toBe(false);

    // Browser forward: a popstate to a page that isn't the one below.
    window.dispatchEvent(new PopStateEvent("popstate"));
    noteRoute("/series/a");
    expect(canGoBack()).toBe(true);
  });
});
