/**
 * `<OnDeckCard>` URL-threading tests for the B-2 audit-finding fix.
 * The home On Deck rail's CBL cards must include `?cbl=<saved_view_id>`
 * on every outbound reader / issue URL so the reader's next-up
 * resolver keeps picking from the list across page turns. Series cards
 * stay unchanged (no CBL context).
 */
import { describe, expect, it, vi } from "vitest";
import type * as React from "react";
import Link from "next/link";

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

import { OnDeckCard } from "@/components/library/OnDeckCard";
import { QuickReadOverlay } from "@/components/QuickReadOverlay";
import type { IssueSummaryView, OnDeckCard as OnDeckCardData } from "@/lib/api/types";

const SV_ID = "00000000-0000-0000-0000-00000000abcd";

function issue(overrides: Partial<IssueSummaryView> = {}): IssueSummaryView {
  return {
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
    ...overrides,
  };
}

function cblCard(
  overrides: Partial<Extract<OnDeckCardData, { kind: "cbl_next" }>> = {},
): OnDeckCardData {
  return {
    kind: "cbl_next",
    issue: issue(),
    cbl_list_id: "00000000-0000-0000-0000-000000000aaa",
    cbl_list_name: "Best of 2003",
    cbl_saved_view_id: SV_ID,
    position: 2,
    last_activity: "2026-01-01T00:00:00Z",
    ...overrides,
  };
}

function seriesCard(): OnDeckCardData {
  return {
    kind: "series_next",
    issue: issue(),
    series_name: "Invincible",
    last_activity: "2026-01-01T00:00:00Z",
  };
}

function findByType(node: React.ReactNode, type: unknown): unknown {
  const stack: React.ReactNode[] = [node];
  while (stack.length) {
    const cur = stack.shift();
    if (Array.isArray(cur)) {
      stack.push(...cur);
      continue;
    }
    if (!cur || typeof cur !== "object" || !("type" in cur)) continue;
    const el = cur as React.ReactElement<{ children?: React.ReactNode }>;
    if (el.type === type) return el;
    if (el.props && el.props.children) stack.push(el.props.children);
  }
  return null;
}

describe("<OnDeckCard> CBL URL threading (B-2)", () => {
  it("cbl_next card with saved-view id includes ?cbl= on the issue Link", () => {
    const tree = OnDeckCard({ card: cblCard() });
    const link = findByType(tree, Link) as React.ReactElement<{
      href?: string;
    }> | null;
    expect(link).toBeTruthy();
    expect(link!.props.href).toContain(`?cbl=${SV_ID}`);
  });

  it("cbl_next card with saved-view id includes ?cbl= on the QuickReadOverlay", () => {
    const tree = OnDeckCard({ card: cblCard() });
    const overlay = findByType(tree, QuickReadOverlay) as React.ReactElement<{
      readerHref?: string;
    }> | null;
    expect(overlay).toBeTruthy();
    expect(overlay!.props.readerHref).toContain(`?cbl=${SV_ID}`);
  });

  it("cbl_next card without saved-view id omits the ?cbl= param", () => {
    const tree = OnDeckCard({
      card: cblCard({ cbl_saved_view_id: undefined }),
    });
    const link = findByType(tree, Link) as React.ReactElement<{
      href?: string;
    }> | null;
    expect(link!.props.href).not.toContain("?cbl=");
  });

  it("series_next card never carries ?cbl=", () => {
    const tree = OnDeckCard({ card: seriesCard() });
    const link = findByType(tree, Link) as React.ReactElement<{
      href?: string;
    }> | null;
    expect(link!.props.href).not.toContain("?cbl=");
  });
});
