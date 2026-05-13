/**
 * Smoke checks for `<CblIssueCard>` — the rail/detail variant of
 * `IssueCard`. We don't render to the DOM (vitest is node-env), so we
 * inspect the React element tree directly. That's sufficient to confirm
 * branching on `match_status` + presence of `issue` produces the right
 * badge/link mix.
 */
import { describe, expect, it, vi } from "vitest";
import type * as React from "react";
import Link from "next/link";

// `CblIssueCard` now calls `useUpsertIssueProgress()` for the kebab
// menu's mark-read/unread actions. Stub the hook so we can render the
// component without a QueryClient in scope.
vi.mock("@/lib/api/mutations", () => ({
  useUpsertIssueProgress: () => ({
    mutate: () => {},
    isPending: false,
  }),
}));
// QuickReadOverlay and CoverMenuButton transitively pull `useRouter`.
vi.mock("next/navigation", () => ({
  useRouter: () => ({ push: () => {} }),
}));
// Markers + Collections M3 added the cover-menu collection actions hook
// to every card variant. Stub it out so the card-tree shape inspection
// doesn't need a real QueryClient.
vi.mock("@/components/collections/useCoverMenuCollectionActions", () => ({
  useCoverMenuCollectionActions: () => ({ actions: [], dialog: null }),
}));
// Touch-platform long-press sheet relies on `useSyncExternalStore` +
// other hooks that can't run inside this node-env tree-inspection
// pattern. Stub the hook to its no-op shape (matches the desktop path
// where the hook returns empty handlers and a null sheet).
vi.mock("@/components/CoverLongPressActions", () => ({
  useCoverLongPressActions: () => ({ wrapperProps: {}, sheet: null }),
}));

import { CblIssueCard } from "@/components/cbl/cbl-issue-card";
import type {
  CblEntryView,
  CblMatchStatus,
  IssueSummaryView,
} from "@/lib/api/types";

function entry(overrides: Partial<CblEntryView> = {}): CblEntryView {
  return {
    id: "e1",
    position: 0,
    series_name: "Invincible",
    issue_number: "1",
    volume: null,
    year: "2003",
    cv_series_id: null,
    cv_issue_id: null,
    matched_issue_id: null,
    match_status: "matched" as CblMatchStatus,
    match_method: null,
    match_confidence: null,
    matched_at: null,
    ambiguous_candidates: null,
    ...overrides,
  };
}

function issue(overrides: Partial<IssueSummaryView> = {}): IssueSummaryView {
  return {
    id: "i1",
    slug: "issue-1",
    series_id: "s1",
    series_slug: "invincible",
    title: "Out of This World",
    number: "1",
    sort_number: 1,
    year: 2003,
    page_count: 22,
    state: "active",
    cover_url: null,
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
    ...overrides,
  };
}

function rootType(node: React.ReactNode): unknown {
  if (!node || typeof node !== "object" || !("type" in node)) return null;
  return (node as React.ReactElement).type;
}

/** Walk the tree and return the first element whose type matches. The
 *  hydrated branch now returns a Fragment containing the Link so a
 *  click on the AddToCollectionDialog can't bubble back to it. */
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

describe("CblIssueCard", () => {
  it("matched + hydrated issue renders a Link to the issue page", () => {
    const tree = CblIssueCard({ entry: entry(), issue: issue() });
    const link = findByType(tree, Link) as React.ReactElement<{
      href?: string;
    }> | null;
    expect(link).toBeTruthy();
    expect(link!.props.href).toContain("/series/invincible/issues/issue-1");
  });

  it("ambiguous entry without a hydrated issue renders as a plain div", () => {
    const tree = CblIssueCard({
      entry: entry({ match_status: "ambiguous" }),
      issue: undefined,
    });
    expect(rootType(tree)).toBe("div");
  });

  it("missing entry without a hydrated issue renders as a plain div", () => {
    const tree = CblIssueCard({
      entry: entry({ match_status: "missing" }),
      issue: undefined,
    });
    expect(rootType(tree)).toBe("div");
  });
});
