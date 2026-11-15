/** `CoverChip` (WP-9.4): the mark that keeps a status from being
 *  colour-only, and the sentence-case helper for raw state tokens. */
import { Lock } from "lucide-react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import { CoverChip, coverChipLabel } from "@/components/library/CoverChip";

describe("CoverChip", () => {
  it("neutral is text alone", () => {
    const html = renderToStaticMarkup(<CoverChip>#34</CoverChip>);
    expect(html).toContain('data-tone="neutral"');
    expect(html).toContain("cover-chip__label");
    expect(html).not.toContain("cover-chip__dot");
    expect(html).not.toContain("<svg");
  });

  it.each(["success", "warning", "info", "error"] as const)(
    "%s draws a dot when no icon is given, so colour is never alone",
    (tone) => {
      const html = renderToStaticMarkup(
        <CoverChip tone={tone}>Wanted</CoverChip>,
      );
      expect(html).toContain(`data-tone="${tone}"`);
      expect(html).toContain('class="cover-chip__dot"');
    },
  );

  it("an icon replaces the dot and is hidden from assistive tech", () => {
    const html = renderToStaticMarkup(
      <CoverChip tone="error" icon={Lock} title="Why">
        Password-protected
      </CoverChip>,
    );
    expect(html).toContain("<svg");
    expect(html).toContain('aria-hidden="true"');
    expect(html).not.toContain("cover-chip__dot");
    expect(html).toContain('title="Why"');
  });

  it("carries no colour or backdrop classes of its own", () => {
    const html = renderToStaticMarkup(
      <CoverChip tone="warning" className="absolute top-2 right-2">
        Meta
      </CoverChip>,
    );
    expect(html).toContain('class="cover-chip absolute top-2 right-2"');
  });
});

describe("coverChipLabel", () => {
  it("puts a raw token in sentence case", () => {
    expect(coverChipLabel("ambiguous")).toBe("Ambiguous");
    expect(coverChipLabel("on_hold")).toBe("On hold");
    expect(coverChipLabel("continuing")).toBe("Continuing");
  });

  it("leaves wording that already has capitals alone", () => {
    expect(coverChipLabel("Ended")).toBe("Ended");
    expect(coverChipLabel("TBA")).toBe("TBA");
  });
});
