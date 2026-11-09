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

  it("never offers Back into the reader", () => {
    noteRoute("/series/b");
    noteRoute("/series/b/issues/1");
    expect(canGoBack()).toBe(true);

    // The reader's exit pushes the issue page, so the entry behind it is the
    // reader: Back would re-open it, and exiting would land here again.
    noteRoute("/read/b/1");
    noteRoute("/series/b/issues/1");
    expect(canGoBack()).toBe(false);

    // The next issue, read straight on: still the reader behind it.
    noteRoute("/read/b/1");
    noteRoute("/read/b/2");
    noteRoute("/series/b/issues/2");
    expect(canGoBack()).toBe(false);

    // Moving on through the breadcrumbs: that page's Back is a normal one.
    noteRoute("/series/b");
    expect(canGoBack()).toBe(true);
  });

  it("never offers Back into sign-in", () => {
    // Sign-in pushes the page you were after.
    noteRoute("/sign-in");
    noteRoute("/series/c");
    expect(canGoBack()).toBe(false);
  });

  it("offers Back onto pages outside the library shell", () => {
    noteRoute("/settings/reading");
    noteRoute("/series/d");
    expect(canGoBack()).toBe(true);
  });
});
