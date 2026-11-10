// @vitest-environment jsdom
/**
 * The drag peek (`lazy.tsx` → `DragTurnPeek`) is code-split and purely
 * decorative: a chunk that fails to load must render nothing rather than
 * throw the reader into its error boundary (where a plain `next/dynamic`
 * rejection would land).
 */
import * as React from "react";
import { describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";

vi.mock("@/app/[locale]/read/[seriesSlug]/[issueSlug]/DragTurnPeek", () => {
  throw new TypeError("Failed to fetch dynamically imported module");
});

import { DragTurnPeek } from "@/app/[locale]/read/[seriesSlug]/[issueSlug]/lazy";

class Boundary extends React.Component<
  { children: React.ReactNode },
  { failed: boolean }
> {
  state = { failed: false };
  static getDerivedStateFromError() {
    return { failed: true };
  }
  render() {
    return this.state.failed ? <p>boundary hit</p> : this.props.children;
  }
}

describe("lazy DragTurnPeek", () => {
  it("renders nothing when its chunk fails to load", async () => {
    const { container } = render(
      <Boundary>
        <div data-testid="track">
          <DragTurnPeek
            side="right"
            pages={[1]}
            issueId="issue-1"
            pageInfos={[]}
            pageUrlVersion={null}
            direction="ltr"
            fitClass=""
            sizes="100vw"
            variants
          />
        </div>
      </Boundary>,
    );
    // Let the rejected import settle.
    await waitFor(() => expect(screen.getByTestId("track")).toBeTruthy());
    await new Promise((r) => setTimeout(r, 20));
    expect(screen.queryByText("boundary hit")).toBeNull();
    expect(screen.getByTestId("track").childElementCount).toBe(0);
    expect(container.textContent).toBe("");
  });
});
