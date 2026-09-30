// @vitest-environment jsdom
/**
 * The global keyboard-shortcuts sheet is a lazy chunk (WP-4.4 reader
 * bundle budget): nothing is mounted until the first open, and the `?`
 * hotkey must still open it once the chunk resolves.
 */
import * as React from "react";
import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";

vi.mock("next/navigation", () => ({
  usePathname: () => "/read/some-series/issue-1",
}));
vi.mock("@/lib/api/queries", () => ({
  useMe: () => ({ data: undefined }),
}));

import { GlobalShortcutsSheet } from "@/components/GlobalShortcutsSheet";

describe("GlobalShortcutsSheet (lazy)", () => {
  it("mounts nothing until opened, then `?` opens the lazy sheet", async () => {
    render(
      <GlobalShortcutsSheet>
        <p>page</p>
      </GlobalShortcutsSheet>,
    );
    expect(screen.queryByRole("dialog")).toBeNull();

    fireEvent.keyDown(window, { key: "?" });

    const dialog = await screen.findByRole("dialog");
    expect(dialog).toBeTruthy();
    expect(await screen.findByText("Keyboard shortcuts")).toBeTruthy();
    // In the reader, the Reader section leads.
    expect(screen.getAllByText("Reader").length).toBeGreaterThan(0);
  });
});
