// @vitest-environment jsdom
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { ProviderCoverImage } from "@/components/library/ProviderCoverImage";

// GCD's cover CDN answers every hotlink with a Cloudflare challenge
// (403); any provider CDN can 404 a stale URL. The candidate card must
// show the grey placeholder, not a broken image + alt text.
describe("ProviderCoverImage", () => {
  const GCD =
    "https://files1.comics.org//img/gcd/covers_by_id/258/w400/258242.jpg";

  it("renders the image while it loads", () => {
    render(<ProviderCoverImage src={GCD} alt="Invincible #1" className="c" />);
    const img = screen.getByAltText("Invincible #1");
    expect(img.getAttribute("src")).toBe(GCD);
    expect(img.getAttribute("referrerpolicy")).toBe("no-referrer");
  });

  it("swaps to the placeholder when the CDN refuses the image", () => {
    render(
      <ProviderCoverImage
        src={GCD}
        alt="Invincible #1"
        className="img"
        placeholderClassName="bg-muted placeholder"
      />,
    );
    fireEvent.error(screen.getByAltText("Invincible #1"));
    expect(screen.queryByAltText("Invincible #1")).toBeNull();
    const ph = screen.getByTestId("provider-cover-placeholder");
    expect(ph.className).toBe("bg-muted placeholder");
    expect(ph.getAttribute("aria-hidden")).toBe("true");
  });

  it("retries when the source changes", () => {
    const { rerender } = render(<ProviderCoverImage src={GCD} alt="cover" />);
    fireEvent.error(screen.getByAltText("cover"));
    expect(screen.queryByAltText("cover")).toBeNull();
    rerender(
      <ProviderCoverImage
        src="https://static.metron.cloud/x.jpg"
        alt="cover"
      />,
    );
    expect(screen.getByAltText("cover")).toBeTruthy();
  });

  it("renders the placeholder when there is no URL", () => {
    render(<ProviderCoverImage src={null} alt="none" className="ph" />);
    expect(screen.getByTestId("provider-cover-placeholder").className).toBe(
      "ph",
    );
  });
});
