/**
 * Smoke checks for `<MetaNeedsBadge>` — the cover "meta" chip that flags a
 * series whose metadata likely needs pulling. We don't render to a DOM
 * (vitest node-env), so we inspect the returned React element directly.
 * Enough to confirm:
 *  - it hides when the tier isn't `needs_metadata` or the cover-dot pref is off;
 *  - the passive (selection-mode) variant is a plain badge, not a button;
 *  - the interactive variant "links to the fix" (B4): role=button that routes
 *    to the series page with `?match=1` and stops the parent <Link> firing.
 */
import { describe, expect, it, vi } from "vitest";
import type * as React from "react";
import type { SeriesView } from "@/lib/api/types";

const pushSpy = vi.fn();
vi.mock("next/navigation", () => ({
  useRouter: () => ({ push: pushSpy }),
}));

// The chip shares the cover-dot opt-out. Default the pref to enabled; a
// single test flips it to assert the suppression path.
const dotState = { enabled: true };
vi.mock("@/components/library/use-cover-collection-dot", () => ({
  useCoverCollectionDot: () => ({
    enabled: dotState.enabled,
    setEnabled: () => {},
  }),
}));

import { MetaNeedsBadge } from "@/components/library/SeriesCard";

function seriesWith(tier: string | null): SeriesView {
  return {
    slug: "doom",
    metadata_completeness_tier: tier,
  } as unknown as SeriesView;
}

describe("MetaNeedsBadge", () => {
  it("renders nothing when the tier isn't needs_metadata", () => {
    expect(
      MetaNeedsBadge({ series: seriesWith("complete"), interactive: true }),
    ).toBeNull();
    expect(
      MetaNeedsBadge({ series: seriesWith(null), interactive: true }),
    ).toBeNull();
  });

  it("renders nothing when the cover-dot pref is disabled", () => {
    dotState.enabled = false;
    try {
      expect(
        MetaNeedsBadge({
          series: seriesWith("needs_metadata"),
          interactive: true,
        }),
      ).toBeNull();
    } finally {
      dotState.enabled = true;
    }
  });

  it("is a passive badge (no button role) while selecting", () => {
    const tree = MetaNeedsBadge({
      series: seriesWith("needs_metadata"),
      interactive: false,
    });
    expect(tree).not.toBeNull();
    const props = (tree as React.ReactElement).props as {
      role?: string;
      onClick?: unknown;
    };
    expect(props.role).toBeUndefined();
    expect(props.onClick).toBeUndefined();
  });

  it("interactive: role=button that routes to the series ?match=1 deep-link", () => {
    pushSpy.mockClear();
    const tree = MetaNeedsBadge({
      series: seriesWith("needs_metadata"),
      interactive: true,
    });
    const props = (tree as React.ReactElement).props as {
      role?: string;
      tabIndex?: number;
      "aria-label"?: string;
      onClick?: (e: React.MouseEvent) => void;
    };
    expect(props.role).toBe("button");
    expect(props.tabIndex).toBe(0);
    expect(props["aria-label"]).toBe("Find metadata — likely incomplete");

    let stopped = false;
    let prevented = false;
    props.onClick?.({
      preventDefault: () => {
        prevented = true;
      },
      stopPropagation: () => {
        stopped = true;
      },
    } as unknown as React.MouseEvent);
    expect(prevented).toBe(true);
    expect(stopped).toBe(true);
    expect(pushSpy).toHaveBeenCalledWith("/series/doom?match=1");
  });

  it("interactive: Enter/Space activate; other keys are ignored", () => {
    const tree = MetaNeedsBadge({
      series: seriesWith("needs_metadata"),
      interactive: true,
    });
    const props = (tree as React.ReactElement).props as {
      onKeyDown?: (e: React.KeyboardEvent) => void;
    };
    for (const c of [
      { key: "Enter", activates: true },
      { key: " ", activates: true },
      { key: "Escape", activates: false },
      { key: "ArrowDown", activates: false },
    ]) {
      pushSpy.mockClear();
      let stopped = false;
      props.onKeyDown?.({
        key: c.key,
        preventDefault: () => {},
        stopPropagation: () => {
          stopped = true;
        },
      } as unknown as React.KeyboardEvent);
      if (c.activates) {
        expect(stopped, `key=${c.key} stopped`).toBe(true);
        expect(pushSpy, `key=${c.key} pushed`).toHaveBeenCalledWith(
          "/series/doom?match=1",
        );
      } else {
        expect(stopped, `key=${c.key} stopped`).toBe(false);
        expect(pushSpy, `key=${c.key} not pushed`).not.toHaveBeenCalled();
      }
    }
  });
});
