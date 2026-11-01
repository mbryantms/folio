// @vitest-environment jsdom
/**
 * `<CoverGallery>` admin cover actions (audit 2026-11 §5 #11): each
 * variant offers "Use as cover" (its accessible name names the variant),
 * the primary tile carries a "Cover" chip, the variant that already is
 * the cover offers nothing, and while a variant is the cover the primary
 * tile offers "Use page cover". Readers see no actions.
 */
import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { IssueCoverRow, IssueCoversResp } from "@/lib/api/types";

let covers: IssueCoversResp;
let role: "admin" | "user" = "admin";
const setMutate = vi.fn();
const clearMutate = vi.fn();

vi.mock("@/lib/api/queries", () => ({
  useIssueCovers: () => ({ data: covers, isLoading: false }),
  useMe: () => ({ data: { role } }),
}));

vi.mock("@/lib/api/mutations", () => ({
  useSetIssueVariantCover: () => ({ mutate: setMutate, isPending: false }),
  useClearIssueVariantCover: () => ({ mutate: clearMutate, isPending: false }),
}));

import { CoverGallery } from "@/components/library/CoverGallery";

const primary: IssueCoverRow = {
  id: "00000000-0000-0000-0000-000000000001",
  issue_id: "abc",
  kind: "primary",
  ordinal: 0,
  source_provider: "comicvine",
  source_external_id: "67890",
  source_url: "https://cdn/primary.jpg",
  image_url: "/issues/abc/covers/00000000-0000-0000-0000-000000000001",
  variant_label: null,
  variant_artist_person_id: null,
  width: null,
  height: null,
  fetched_at: "2026-10-31T00:00:00Z",
  is_active: true,
};

const variantB: IssueCoverRow = {
  ...primary,
  id: "00000000-0000-0000-0000-000000000002",
  kind: "variant",
  ordinal: 1,
  source_provider: "metron",
  source_url: "https://cdn/variant-b.jpg",
  image_url: "/issues/abc/covers/00000000-0000-0000-0000-000000000002",
  variant_label: "Cover B (Adam Hughes)",
};

const variantC: IssueCoverRow = {
  ...variantB,
  id: "00000000-0000-0000-0000-000000000003",
  ordinal: 2,
  source_url: "https://cdn/variant-c.jpg",
  image_url: "/issues/abc/covers/00000000-0000-0000-0000-000000000003",
  variant_label: "Cover C",
};

function renderGallery() {
  return render(
    <CoverGallery
      issueId="abc"
      chrome="bare"
      seriesSlug="saga"
      issueSlug="1"
    />,
  );
}

describe("<CoverGallery> cover actions", () => {
  beforeEach(() => {
    role = "admin";
    setMutate.mockReset();
    clearMutate.mockReset();
    covers = {
      issue_id: "abc",
      covers: [primary, variantB, variantC],
      fallback_primary_url: "/x",
      variant_cover_chosen: false,
    };
  });

  it("offers Use as cover on each variant and marks the primary", () => {
    renderGallery();
    expect(screen.getByText("Cover")).toBeTruthy();
    const use = screen.getByRole("button", {
      name: "Use as cover: Cover B (Adam Hughes)",
    });
    expect(
      screen.getByRole("button", { name: "Use as cover: Cover C" }),
    ).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Use page cover" })).toBeNull();
    fireEvent.click(use);
    expect(setMutate).toHaveBeenCalledWith({ variantId: variantB.id });
  });

  it("hides the action on the variant that already is the cover and offers the page cover", () => {
    covers = {
      ...covers,
      covers: [
        {
          ...primary,
          source_provider: "metron",
          source_url: variantB.source_url,
          variant_label: variantB.variant_label,
        },
        variantB,
        variantC,
      ],
      variant_cover_chosen: true,
    };
    renderGallery();
    expect(
      screen.queryByRole("button", {
        name: "Use as cover: Cover B (Adam Hughes)",
      }),
    ).toBeNull();
    expect(
      screen.getByRole("button", { name: "Use as cover: Cover C" }),
    ).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Use page cover" }));
    expect(clearMutate).toHaveBeenCalledTimes(1);
  });

  it("shows readers no cover actions", () => {
    role = "user";
    covers = { ...covers, variant_cover_chosen: true };
    renderGallery();
    expect(screen.queryByRole("button", { name: /Use as cover/ })).toBeNull();
    expect(screen.queryByRole("button", { name: "Use page cover" })).toBeNull();
    expect(screen.getByText("Cover")).toBeTruthy();
  });
});
