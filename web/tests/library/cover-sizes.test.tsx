/**
 * `<Cover>`'s `sizes` (D18). Lazy covers keep `sizes="auto"` (the browser
 * measures the box); priority covers are eager, where `auto` does not
 * apply, so they carry the static card-width hint instead of defaulting to
 * the viewport width (which made every first-rail cover pull the 600px
 * variant at 1x).
 */
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import { Cover, coverSizes } from "@/components/Cover";
import { CoverPriorityProvider } from "@/components/library/cover-priority";
import { COVER_CARD_SIZES } from "@/lib/library/cover-size";

const THUMB = "/issues/abc/pages/0/thumb";

function sizesOf(html: string): string | null {
  return /sizes="([^"]*)"/.exec(html)?.[1] ?? null;
}

describe("Cover sizes", () => {
  it("a lazy thumb cover keeps sizes=auto", () => {
    const html = renderToStaticMarkup(<Cover src={THUMB} alt="c" />);
    expect(sizesOf(html)).toBe("auto");
    expect(html).toContain('loading="lazy"');
  });

  it("a priority thumb cover gets the card-width sizes, not the viewport default", () => {
    const html = renderToStaticMarkup(
      <CoverPriorityProvider value>
        <Cover src={THUMB} alt="c" />
      </CoverPriorityProvider>,
    );
    expect(sizesOf(html)).toBe(COVER_CARD_SIZES);
    expect(html).toContain('loading="eager"');
  });

  it("a cover without a srcset (provider URL) gets no sizes", () => {
    const html = renderToStaticMarkup(
      <CoverPriorityProvider value>
        <Cover src="/covers/xyz" alt="c" />
      </CoverPriorityProvider>,
    );
    expect(sizesOf(html)).toBeNull();
    expect(coverSizes(undefined, true)).toBeUndefined();
  });

  it("the static hint keeps a default desktop card on the 300w variant at 1x", () => {
    // Desktop slot is a fixed px value ≤ 300, so at DPR 1 the browser's
    // pick from `…300w, …600w` is the 300 one.
    const desktop = /(\d+)px$/.exec(COVER_CARD_SIZES)?.[1];
    expect(Number(desktop)).toBeLessThanOrEqual(300);
    expect(Number(desktop)).toBeGreaterThanOrEqual(160);
  });
});
