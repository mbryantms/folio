// @vitest-environment jsdom
/**
 * `<CoverPagePickerDialog>` (post-audit T4.2): one tile per page, the
 * current cover is marked and not clickable, clicking another tile
 * reports its 0-based index, and the grid locks while a pick applies.
 */
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { CoverPagePickerDialog } from "@/components/library/CoverPagePickerDialog";

function renderDialog(
  overrides: Partial<Parameters<typeof CoverPagePickerDialog>[0]> = {},
) {
  const onPick = vi.fn();
  render(
    <CoverPagePickerDialog
      open
      onOpenChange={() => undefined}
      issueId="issue-1"
      pageCount={4}
      currentPageIndex={0}
      onPick={onPick}
      {...overrides}
    />,
  );
  return { onPick };
}

describe("<CoverPagePickerDialog>", () => {
  it("renders one tile per page and marks the current cover", () => {
    renderDialog();
    const tiles = screen.getAllByRole("button", { name: /^Page \d/ });
    expect(tiles).toHaveLength(4);
    const current = screen.getByRole("button", {
      name: "Page 1 (current cover)",
    });
    expect(current.getAttribute("aria-pressed")).toBe("true");
    expect((current as HTMLButtonElement).disabled).toBe(true);
    // Decorative thumbs (`alt=""`) carry no img role; read them off the DOM.
    const thumbs = document.querySelectorAll<HTMLImageElement>("li img");
    expect(thumbs).toHaveLength(4);
    expect(thumbs[2].getAttribute("src")).toBe(
      "/issues/issue-1/pages/2/thumb?variant=strip",
    );
  });

  it("reports the 0-based index of the clicked page", () => {
    const { onPick } = renderDialog({ currentPageIndex: 1 });
    fireEvent.click(screen.getByRole("button", { name: "Page 3" }));
    expect(onPick).toHaveBeenCalledWith(2);
  });

  it("locks every tile while a pick is applying", () => {
    const { onPick } = renderDialog({ isPending: true });
    fireEvent.click(screen.getByRole("button", { name: "Page 2" }));
    expect(onPick).not.toHaveBeenCalled();
    expect(
      screen.getByRole("list", { name: "Pages" }).getAttribute("aria-busy"),
    ).toBe("true");
  });

  it("explains an empty archive instead of rendering a grid", () => {
    renderDialog({ pageCount: 0 });
    expect(screen.queryByRole("list", { name: "Pages" })).toBeNull();
    expect(screen.getByText(/no pages to choose from/i)).toBeTruthy();
  });
});
