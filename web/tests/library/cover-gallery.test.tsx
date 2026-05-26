/**
 * <CoverGallery> smoke — metadata-providers-1.0 M5.2.
 *
 * Verifies the gallery's "hide when nothing variant" rule + that a
 * variant row renders the source_url as an <img>.
 */
import { describe, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { createElement } from "react";
import type { IssueCoverRow } from "@/lib/api/types";

let queryState: {
  data:
    | undefined
    | {
        issue_id: string;
        covers: IssueCoverRow[];
        fallback_primary_url: string;
      };
  isLoading: boolean;
} = { data: undefined, isLoading: false };

vi.mock("@/lib/api/queries", () => ({
  useIssueCovers: () => queryState,
}));

vi.mock("next/image", () => ({
  default: ({ src, alt }: { src: string; alt: string }) =>
    createElement("img", { src, alt }),
}));

import { CoverGallery } from "@/components/library/CoverGallery";

const primary: IssueCoverRow = {
  id: "00000000-0000-0000-0000-000000000001",
  issue_id: "abc",
  kind: "primary",
  ordinal: 0,
  source_provider: "comicvine",
  source_external_id: "67890",
  source_url: "https://cdn/super.jpg",
  variant_label: null,
  variant_artist_person_id: null,
  width: null,
  height: null,
  fetched_at: "2026-05-26T00:00:00Z",
  is_active: true,
};

const variant: IssueCoverRow = {
  ...primary,
  id: "00000000-0000-0000-0000-000000000002",
  kind: "variant",
  ordinal: 1,
  source_url: "https://cdn/variant-b.jpg",
  variant_label: "Cover B (Adam Hughes)",
};

describe("<CoverGallery>", () => {
  it("renders nothing when there are no rows", () => {
    queryState = {
      data: { issue_id: "abc", covers: [], fallback_primary_url: "/x" },
      isLoading: false,
    };
    const html = renderToStaticMarkup(
      createElement(CoverGallery, { issueId: "abc" }),
    );
    expect(html).toBe("");
  });

  it("renders nothing when only the primary cover exists (header already shows it)", () => {
    queryState = {
      data: {
        issue_id: "abc",
        covers: [primary],
        fallback_primary_url: "/x",
      },
      isLoading: false,
    };
    const html = renderToStaticMarkup(
      createElement(CoverGallery, { issueId: "abc" }),
    );
    expect(html).toBe("");
  });

  it("renders the grid when at least one variant exists", () => {
    queryState = {
      data: {
        issue_id: "abc",
        covers: [primary, variant],
        fallback_primary_url: "/x",
      },
      isLoading: false,
    };
    const html = renderToStaticMarkup(
      createElement(CoverGallery, { issueId: "abc" }),
    );
    expect(html).toContain("Covers");
    expect(html).toContain("Cover B (Adam Hughes)");
    expect(html).toContain('src="https://cdn/variant-b.jpg"');
    expect(html).toContain("ComicVine");
  });

  it("renders the loading shell while pending", () => {
    queryState = { data: undefined, isLoading: true };
    const html = renderToStaticMarkup(
      createElement(CoverGallery, { issueId: "abc" }),
    );
    expect(html).toContain("Loading");
  });
});
