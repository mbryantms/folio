// @vitest-environment jsdom
/**
 * Post-audit T2.1 (D1): the Continue Reading rail's card resumes the
 * issue on its primary tap and keeps the detail page as a secondary
 * link; everywhere else the card's primary target is the detail page.
 */
import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";

vi.mock("next/navigation", () => ({
  useRouter: () => ({ push: () => {} }),
}));
vi.mock("@/lib/api/mutations", () => ({
  useDismissRailItem: () => ({ mutate: () => {}, isPending: false }),
  useUpsertIssueProgress: () => ({ mutate: () => {}, isPending: false }),
}));
vi.mock("@/components/CoverLongPressActions", () => ({
  useCoverLongPressActions: () => ({ wrapperProps: {}, sheet: null }),
}));
vi.mock("@/components/collections/useCoverMenuCollectionActions", () => ({
  useCoverMenuCollectionActions: () => ({ actions: [], dialog: null }),
}));
vi.mock("@/components/CoverMenuButton", () => ({
  CoverMenuButton: ({ actions }: { actions: { label: string }[] }) => (
    <ul data-testid="menu">
      {actions.map((a) => (
        <li key={a.label}>{a.label}</li>
      ))}
    </ul>
  ),
}));

import { ProgressIssueCard } from "@/components/library/ProgressIssueCard";
import type { ContinueReadingCard } from "@/lib/api/types";

const card: ContinueReadingCard = {
  issue: {
    id: "i1",
    slug: "issue-2",
    series_id: "s1",
    series_slug: "invincible",
    series_name: "Invincible",
    title: "Eight Is Enough",
    number: "2",
    sort_number: 2,
    year: 2003,
    page_count: 22,
    state: "active",
    cover_url: null,
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
  },
  series_name: "Invincible",
  progress: {
    last_page: 4,
    percent: 0.2,
    updated_at: "2026-01-01T00:00:00Z",
  },
} as unknown as ContinueReadingCard;

describe("<ProgressIssueCard>", () => {
  it("resumes in the reader on the rail and links Details separately", () => {
    render(<ProgressIssueCard card={card} primaryAction="reader" />);
    // Named from content behind a hidden "Continue reading" prefix, so
    // the visible title is part of the name (axe
    // `label-content-name-mismatch`).
    const primary = screen.getByRole("link", {
      name: /^Continue reading Eight Is Enough/,
    });
    expect(primary.getAttribute("href")).toBe("/read/invincible/issue-2");
    const details = screen.getByRole("link", { name: "Details" });
    expect(details.getAttribute("href")).toBe(
      "/series/invincible/issues/issue-2",
    );
    // The sheet / kebab also offers the detail page.
    expect(screen.getByTestId("menu").textContent).toMatch(/Issue details/);
  });

  it("keeps the detail page as the primary target by default", () => {
    render(<ProgressIssueCard card={card} />);
    const primary = screen.getByRole("link", {
      name: /^Eight Is Enough/,
    });
    expect(primary.getAttribute("href")).toBe(
      "/series/invincible/issues/issue-2",
    );
    expect(screen.queryByRole("link", { name: "Details" })).toBeNull();
    expect(screen.getByTestId("menu").textContent).not.toMatch(/Issue details/);
  });
});
