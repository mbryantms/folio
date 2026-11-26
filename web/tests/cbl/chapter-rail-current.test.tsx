// @vitest-environment jsdom
/**
 * The chapter rail marks the chapter in view (`aria-current="location"`)
 * and moves the mark when the current chapter changes; a click reports
 * the chapter to jump to.
 */
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { ChapterRailList } from "@/components/cbl/links/ChapterParts";
import type { ReadingListChapterView } from "@/lib/api/types";

function chapter(index: number, label: string): ReadingListChapterView {
  return {
    index,
    start_position: index * 10,
    end_position: index * 10 + 9,
    label,
    event_link_ids: [],
    total: 10,
    in_library: 10,
    read: 0,
  };
}

const chapters = [
  chapter(0, "Opening"),
  chapter(1, "Middle"),
  chapter(2, "End"),
];

function rail(current: number, onJump = vi.fn()) {
  return (
    <ChapterRailList
      chapters={chapters}
      events={[]}
      next={null}
      current={current}
      onJump={onJump}
    />
  );
}

const button = (name: RegExp) => screen.getByRole("button", { name });

describe("<ChapterRailList> current chapter", () => {
  it("marks only the chapter in view", () => {
    render(rail(1));
    expect(button(/Middle/).getAttribute("aria-current")).toBe("location");
    expect(button(/Opening/).hasAttribute("aria-current")).toBe(false);
    expect(button(/End/).hasAttribute("aria-current")).toBe(false);
  });

  it("moves the mark when the page reaches another chapter", () => {
    const { rerender } = render(rail(0));
    expect(button(/Opening/).getAttribute("aria-current")).toBe("location");
    rerender(rail(2));
    expect(button(/End/).getAttribute("aria-current")).toBe("location");
    expect(button(/Opening/).hasAttribute("aria-current")).toBe(false);
  });

  it("reports the chapter clicked", () => {
    const onJump = vi.fn();
    render(rail(0, onJump));
    fireEvent.click(button(/End/));
    expect(onJump).toHaveBeenCalledWith(2);
  });

  it("keeps the indicator out of the list (an <ol> holds only <li>)", () => {
    const { container } = render(rail(0));
    const list = container.querySelector("ol")!;
    expect([...list.children].every((c) => c.tagName === "LI")).toBe(true);
  });
});
