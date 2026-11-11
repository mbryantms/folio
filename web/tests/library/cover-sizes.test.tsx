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

  it("the static hint is the largest card, so it never under-serves", () => {
    // Desktop: the 300px stop is the widest card; at DPR 1 that still
    // picks 300w, above it 600w. Phone: never fewer than two per row.
    expect(COVER_CARD_SIZES).toBe("(max-width: 639px) 50vw, 300px");
  });
});
