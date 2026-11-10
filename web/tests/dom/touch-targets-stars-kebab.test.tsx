// @vitest-environment jsdom
/**
 * Touch targets for the card kebab, the rating stars and the reader's page
 * scrubber (docs/dev/phone-layout.md rule 9). jsdom cannot evaluate
 * `(pointer: coarse)` or measure pseudo-elements, so these guard the
 * hooks: the shared `folio-tap` class, the side-overrides the CSS reads,
 * and that roles, names and keyboard behaviour are unchanged. The real
 * 44px areas are only verifiable on a touch device.
 */
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { PageScrubber } from "@/app/[locale]/read/[seriesSlug]/[issueSlug]/PageScrubber";
import { CoverMenuButton } from "@/components/CoverMenuButton";
import { RatingStars } from "@/components/folio/rating-stars";

describe("folio-tap side overrides", () => {
  it("the ::after reads --folio-tap-l / -r / -x and defaults to 4px", () => {
    const css = readFileSync(join(process.cwd(), "styles/globals.css"), "utf8");
    const at = css.indexOf("@media (pointer: coarse)");
    const block = css.slice(at, css.indexOf("\n}\n", at));
    expect(block).toMatch(
      /right:\s*calc\(-1 \* var\(--folio-tap-r, var\(--folio-tap-x, 4px\)\)\);/,
    );
    expect(block).toMatch(
      /left:\s*calc\(-1 \* var\(--folio-tap-l, var\(--folio-tap-x, 4px\)\)\);/,
    );
  });
});

describe("card action (cover kebab)", () => {
  it("carries folio-tap and keeps its 32px box", () => {
    render(
      <CoverMenuButton
        label="Actions for Saga"
        actions={[{ label: "Mark read", onSelect: () => {} }]}
      />,
    );
    const kebab = screen.getByRole("button", { name: "Actions for Saga" });
    expect(kebab.classList.contains("folio-tap")).toBe(true);
    expect(kebab.className).toMatch(/\bsize-8\b/);
  });
});

describe("RatingStars touch cells", () => {
  it("every half-star zone carries folio-tap, widened outward only", () => {
    render(<RatingStars value={2} onChange={() => {}} />);
    const radios = screen.getAllByRole("radio");
    expect(radios.map((r) => r.getAttribute("aria-label"))).toEqual([
      "0.5 stars",
      "1 stars",
      "1.5 stars",
      "2 stars",
      "2.5 stars",
      "3 stars",
      "3.5 stars",
      "4 stars",
      "4.5 stars",
      "5 stars",
    ]);
    radios.forEach((r, i) => {
      expect(r.classList.contains("folio-tap")).toBe(true);
      // Left half reaches left into the gap, right half right; the
      // halves meet at the midline (0px toward each other).
      if (i % 2 === 0) {
        expect(r.className).toContain("[--folio-tap-l:1px]");
        expect(r.className).toContain("[--folio-tap-r:0px]");
      } else {
        expect(r.className).toContain("[--folio-tap-l:0px]");
        expect(r.className).toContain("[--folio-tap-r:1px]");
      }
      expect(r.getAttribute("tabindex")).toBe("-1");
    });
    expect(
      screen
        .getByRole("radio", { name: "2 stars" })
        .getAttribute("aria-checked"),
    ).toBe("true");
  });

  it("the large size reaches half its 4px gap", () => {
    render(<RatingStars value={null} onChange={() => {}} size="lg" />);
    const [left, right] = screen.getAllByRole("radio");
    expect(left!.className).toContain("[--folio-tap-l:2px]");
    expect(right!.className).toContain("[--folio-tap-r:2px]");
  });

  it("keeps the group name, half/whole clicks and keyboard", () => {
    const onChange = vi.fn();
    const { rerender } = render(
      <RatingStars value={null} onChange={onChange} label="Your rating" />,
    );
    const group = screen.getByRole("radiogroup", {
      name: "Your rating: 0 of 5 stars",
    });
    fireEvent.click(screen.getByRole("radio", { name: "3.5 stars" }));
    expect(onChange).toHaveBeenLastCalledWith(3.5);
    fireEvent.click(screen.getByRole("radio", { name: "4 stars" }));
    expect(onChange).toHaveBeenLastCalledWith(4);
    fireEvent.keyDown(group, { key: "ArrowRight" });
    expect(onChange).toHaveBeenLastCalledWith(0.5);
    fireEvent.keyDown(group, { key: "3", shiftKey: true });
    expect(onChange).toHaveBeenLastCalledWith(2.5);
    // Clicking the current value clears it.
    rerender(<RatingStars value={4} onChange={onChange} label="Your rating" />);
    fireEvent.click(screen.getByRole("radio", { name: "4 stars" }));
    expect(onChange).toHaveBeenLastCalledWith(null);
    expect(group.getAttribute("tabindex")).toBe("0");
  });

  it("read-only renders no hit zones", () => {
    render(<RatingStars value={3} readOnly />);
    expect(screen.queryAllByRole("radio")).toHaveLength(0);
  });
});

describe("PageScrubber on touch", () => {
  it("starts 2px lower so the header tap areas keep half the overlap", () => {
    render(
      <PageScrubber
        issueId="iss"
        totalPages={11}
        currentPage={3}
        direction="ltr"
        urlVersion={null}
      />,
    );
    const slider = screen.getByRole("slider", { name: "Page" });
    // 30px instead of 32px, bottom edge unchanged.
    expect(slider.className).toContain("pointer-coarse:h-7.5");
    expect(slider.className).toContain("pointer-coarse:-bottom-6");
    expect(slider.className).not.toContain("pointer-coarse:h-8");
    // Rail and knob move up by the same 2px, so they stay where they were.
    const [rail, knob] = [...slider.querySelectorAll("span")];
    expect(rail!.className).toContain("pointer-coarse:top-1");
    expect(knob!.className).toContain("pointer-coarse:top-1.5");
  });
});
