/**
 * Smoke checks for `<QuickReadOverlay>`. We don't render to a DOM (vitest
 * node-env), so we inspect the returned React element directly. Enough
 * to confirm the overlay renders a `Link` pointing at the reader href,
 * carries the right aria-label, and stops click propagation so it
 * doesn't trigger the parent card's `<Link>` to the detail page.
 */
import { describe, expect, it, vi } from "vitest";
import type * as React from "react";

// `QuickReadOverlay` calls `useRouter()` at mount-time. We aren't rendering
// to a real DOM (vitest node-env), so we stub the hook with a recorder we
// can assert against.
const pushSpy = vi.fn();
vi.mock("next/navigation", () => ({
  useRouter: () => ({ push: pushSpy }),
}));

import { QuickReadOverlay } from "@/components/QuickReadOverlay";

describe("QuickReadOverlay", () => {
  it("renders a span with role=button + aria-label", () => {
    const tree = QuickReadOverlay({
      readerHref: "/read/saga/chapter-one",
      label: "Continue reading Saga #1",
    });
    expect(tree.type).toBe("span");
    const props = tree.props as {
      role?: string;
      tabIndex?: number;
      "aria-label"?: string;
    };
    expect(props.role).toBe("button");
    expect(props.tabIndex).toBe(0);
    expect(props["aria-label"]).toBe("Continue reading Saga #1");
  });

  it("clicking routes to the reader href + stops propagation + prevents default", () => {
    pushSpy.mockClear();
    const tree = QuickReadOverlay({
      readerHref: "/read/a/b",
      label: "Read",
    });
    const props = tree.props as {
      onClick?: (e: React.MouseEvent) => void;
    };
    let stopped = false;
    let prevented = false;
    const fakeEvent = {
      preventDefault: () => {
        prevented = true;
      },
      stopPropagation: () => {
        stopped = true;
      },
    } as unknown as React.MouseEvent;
    props.onClick?.(fakeEvent);
    expect(prevented).toBe(true);
    expect(stopped).toBe(true);
    expect(pushSpy).toHaveBeenCalledWith("/read/a/b");
  });

  it("Enter and Space activate; other keys are ignored", () => {
    pushSpy.mockClear();
    const tree = QuickReadOverlay({
      readerHref: "/read/a/b",
      label: "Read",
    });
    const props = tree.props as {
      onKeyDown?: (e: React.KeyboardEvent) => void;
    };
    const cases: { key: string; activates: boolean }[] = [
      { key: "Enter", activates: true },
      { key: " ", activates: true },
      { key: "Escape", activates: false },
      { key: "ArrowDown", activates: false },
    ];
    for (const c of cases) {
      pushSpy.mockClear();
      let stopped = false;
      const fakeEvent = {
        key: c.key,
        preventDefault: () => {},
        stopPropagation: () => {
          stopped = true;
        },
      } as unknown as React.KeyboardEvent;
      props.onKeyDown?.(fakeEvent);
      if (c.activates) {
        expect(stopped, `key=${c.key} stopped`).toBe(true);
        expect(pushSpy, `key=${c.key} pushed`).toHaveBeenCalledWith(
          "/read/a/b",
        );
      } else {
        expect(stopped, `key=${c.key} stopped`).toBe(false);
        expect(pushSpy, `key=${c.key} not pushed`).not.toHaveBeenCalled();
      }
    }
  });
});
