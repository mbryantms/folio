// @vitest-environment jsdom
/**
 * An untitled issue's card prints its number once. Regression: on the
 * series page (whose issue list carries no `series_name`) the card read
 * "#3" over "#3", and its collection dialog said "#3 #3".
 */
import { render } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

const collectionLabels: string[] = [];

vi.mock("next/navigation", () => ({
  useRouter: () => ({ push: () => {} }),
}));
vi.mock("@/lib/visual-effects", () => ({
  useVisualEffect: () => false,
  useReducedMotion: () => true,
}));
vi.mock("@/lib/api/mutations", () => ({
  useUpsertIssueProgress: () => ({ mutate: () => {}, isPending: false }),
}));
vi.mock("@/lib/api/queries", () => ({
  useUserProgress: () => ({ data: undefined }),
}));
vi.mock("@/components/CoverLongPressActions", () => ({
  useCoverLongPressActions: () => ({ wrapperProps: {}, sheet: null }),
}));
vi.mock("@/components/collections/useCoverMenuCollectionActions", () => ({
  useCoverMenuCollectionActions: ({ label }: { label: string }) => {
    collectionLabels.push(label);
    return { actions: [], dialog: null };
  },
}));
vi.mock("@/components/CoverMenuButton", () => ({
  CoverMenuButton: () => null,
}));
vi.mock("@/components/library/PageScrub", () => ({
  PageScrub: () => null,
}));
vi.mock("@/lib/ui/use-share-link", () => ({
  useShareLink: () => ({ label: "Share", shareOrCopy: () => {} }),
}));

import { IssueCard } from "@/components/library/IssueCard";
import type { IssueSummaryView } from "@/lib/api/types";

function issue(overrides: Partial<IssueSummaryView>): IssueSummaryView {
  return {
    id: "i3",
    slug: "issue-3",
    series_id: "s1",
    series_slug: "saga",
    title: null,
    number: "3",
    sort_number: 3,
    year: null,
    page_count: 20,
    state: "active",
    cover_url: null,
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
    ...overrides,
  };
}

function visibleText(el: HTMLElement): string {
  // The text block under the cover (number line + title line).
  const block = el.querySelector(".min-w-0.px-1");
  return block?.textContent ?? "";
}

describe("<IssueCard> label", () => {
  it("shows an untitled issue's number once", () => {
    collectionLabels.length = 0;
    const { container } = render(<IssueCard issue={issue({})} />);
    const text = visibleText(container);
    expect(text.match(/#3/g)).toHaveLength(1);
    expect(collectionLabels.at(-1)).toBe("#3");
  });

  it("shows the title under the number for a titled issue", () => {
    collectionLabels.length = 0;
    const { container } = render(
      <IssueCard issue={issue({ title: "Chapter Three" })} />,
    );
    expect(visibleText(container)).toBe("#3Chapter Three");
    expect(collectionLabels.at(-1)).toBe("Chapter Three #3");
  });
});
