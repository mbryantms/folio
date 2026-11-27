// @vitest-environment jsdom
/**
 * Pages Folio kept from a damaged archive are called out, never hidden:
 * the issue page's chip names each page and what to do, and the reader's
 * notice shows while one is on screen.
 */
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { DamagedPageNotice } from "@/app/[locale]/read/[seriesSlug]/[issueSlug]/DamagedPageNotice";
import { DamagedPagesBadge } from "@/components/library/DamagedPagesBadge";
import { useReaderStore } from "@/lib/reader/store";
import type { DamagedPageView } from "@/lib/api/types";
import { damagedPageAt, damagedPageLine } from "@/lib/library/damaged-pages";

const salvaged: DamagedPageView = {
  page_index: 7,
  kind: "salvaged",
  reason: "checksum-failed",
  detected_at: "2026-11-27T00:00:00Z",
};
const placeholder: DamagedPageView = {
  ...salvaged,
  page_index: 1,
  kind: "placeholder",
};

describe("damaged pages", () => {
  it("describes each kind with 1-based page numbers", () => {
    expect(damagedPageLine(salvaged)).toMatch(/^Page 8: partly recovered/);
    expect(damagedPageLine(placeholder)).toMatch(
      /^Page 2: nothing could be recovered/,
    );
    expect(damagedPageAt([salvaged, placeholder], 1)).toBe(placeholder);
    expect(damagedPageAt([salvaged], 0)).toBeUndefined();
  });

  it("renders no chip for an undamaged issue", () => {
    const { container } = render(<DamagedPagesBadge pages={[]} />);
    expect(container.innerHTML).toBe("");
  });

  it("opens a list of the damaged pages from the chip", () => {
    render(<DamagedPagesBadge pages={[placeholder, salvaged]} />);
    const chip = screen.getByTestId("damaged-pages-badge");
    expect(chip.textContent).toBe("2 damaged pages");
    fireEvent.click(chip);
    expect(
      screen.getByText(/^Page 2: nothing could be recovered/),
    ).toBeTruthy();
    expect(screen.getByText(/^Page 8: partly recovered/)).toBeTruthy();
    expect(screen.getByText(/Replace the file with a good copy/)).toBeTruthy();
  });

  it("says in the reader which page on screen is damaged", () => {
    useReaderStore.setState({ currentPage: 7, viewMode: "single" });
    render(<DamagedPageNotice damagedPages={[placeholder, salvaged]} />);
    expect(screen.getByRole("status").textContent).toBe(
      "Page 8 is damaged in the file: shown as recovered.",
    );
  });

  it("shows nothing while no damaged page is on screen", () => {
    useReaderStore.setState({ currentPage: 3, viewMode: "single" });
    const { container } = render(
      <DamagedPageNotice damagedPages={[placeholder, salvaged]} />,
    );
    expect(container.innerHTML).toBe("");
  });

  it("counts the right-hand page of a spread as on screen", () => {
    useReaderStore.setState({ currentPage: 0, viewMode: "double" });
    render(<DamagedPageNotice damagedPages={[placeholder]} />);
    expect(screen.getByRole("status").textContent).toBe(
      "Page 2 is damaged in the file: nothing could be recovered.",
    );
  });
});
