/**
 * Smoke tests for `<EndOfIssueCard>` — covers the three states the card
 * renders (loading skeleton, "Up next" with target, "You're caught up").
 * Uses `renderToStaticMarkup` to flatten the tree to HTML so the
 * inspection reaches sub-component output without needing a DOM
 * environment, QueryClient, or router.
 */
import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { createElement } from "react";

import { EndOfIssueCard } from "@/app/[locale]/read/[seriesSlug]/[issueSlug]/EndOfIssueCard";
import type { IssueSummaryView, NextUpView } from "@/lib/api/types";

function issue(overrides: Partial<IssueSummaryView> = {}): IssueSummaryView {
  return {
    id: "i2",
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
    cover_url: "/issues/i2/pages/0/thumb",
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
    ...overrides,
  };
}

const noop = () => undefined;

function render(
  partial: Partial<Parameters<typeof EndOfIssueCard>[0]> = {},
): string {
  const props: Parameters<typeof EndOfIssueCard>[0] = {
    open: true,
    data: undefined,
    isLoading: false,
    direction: "ltr",
    exitUrl: "/series/x/issues/y",
    onContinue: noop,
    onDismiss: noop,
    ...partial,
  };
  // Go through React's render path (not a direct function call) so the
  // component's hooks (useRef / useEffect) get a renderer context.
  return renderToStaticMarkup(createElement(EndOfIssueCard, props));
}

describe("<EndOfIssueCard>", () => {
  it("renders a loading state while the resolver is in-flight", () => {
    const html = render({ isLoading: true });
    // Skeleton cover has the aria-busy + label.
    expect(html).toMatch(/Loading next issue/i);
  });

  it("renders the up-next body for source = series", () => {
    const data: NextUpView = { source: "series", target: issue() };
    const html = render({ data });
    expect(html).toMatch(/Next up/i);
    expect(html).toMatch(/Eight Is Enough/);
    // Single primary CTA per the condensed design — labeled "Read".
    expect(html).toMatch(/Read/);
    // Close affordance for "stay here" semantics.
    expect(html).toMatch(/aria-label="Close"/);
    // The up-next body does NOT render Exit reader / Browse buttons —
    // those are only on the caught-up state.
    expect(html).not.toMatch(/Exit reader/);
    expect(html).not.toMatch(/Browse the library/);
  });

  it("renders the CBL subtitle when CBL fields are present", () => {
    const data: NextUpView = {
      source: "cbl",
      target: issue(),
      cbl_list_id: "00000000-0000-0000-0000-000000000aaa",
      cbl_list_name: "Best of 2003",
      cbl_position: 4,
      cbl_total: 24,
    };
    const html = render({ data });
    expect(html).toMatch(/Issue 4 of 24 in Best of 2003/);
    expect(html).toMatch(/Read/);
  });

  it("renders the caught-up body when source = none", () => {
    const data: NextUpView = { source: "none" };
    const html = render({ data });
    expect(html).toMatch(/caught up/i);
    // Caught-up state surfaces Browse-the-library + Exit reader,
    // never a "Read" primary (nothing to read next).
    expect(html).toMatch(/Browse the library/);
    expect(html).toMatch(/Exit reader/);
  });

  it("renders the fallback suggestion tile when source=none and fallback present (D-6)", () => {
    const data: NextUpView = {
      source: "none",
      fallback_suggestion: {
        kind: "series_next",
        issue: issue({
          id: "sugg1",
          slug: "suggested-1",
          series_slug: "suggested-series",
          series_name: "Suggested",
          title: "Suggested Issue",
        }),
        series_name: "Suggested",
        last_activity: "2026-01-01T00:00:00Z",
      },
    };
    const html = render({ data });
    expect(html).toMatch(/here's a suggestion|here&#x27;s a suggestion/);
    expect(html).toMatch(/Suggested Issue/);
    // The tile is a <Link> to the reader URL.
    expect(html).toMatch(/\/read\/suggested-series\/suggested-1/);
  });

  it("threads cbl context onto the fallback tile when source=none and fallback is cbl_next", () => {
    const SV_ID = "00000000-0000-0000-0000-00000000abcd";
    const data: NextUpView = {
      source: "none",
      fallback_suggestion: {
        kind: "cbl_next",
        issue: issue({
          id: "cbl-sugg1",
          slug: "cbl-suggested-1",
          series_slug: "x",
        }),
        cbl_list_id: "00000000-0000-0000-0000-000000000aaa",
        cbl_list_name: "My CBL",
        cbl_saved_view_id: SV_ID,
        position: 4,
        last_activity: "2026-01-01T00:00:00Z",
      },
    };
    const html = render({ data });
    expect(html).toMatch(/In My CBL · entry 4/);
    expect(html).toMatch(`?cbl=${SV_ID}`);
  });

  it("falls back to the plain caught-up message when source=none and no fallback present", () => {
    const data: NextUpView = { source: "none" };
    const html = render({ data });
    expect(html).toMatch(/No next issue to suggest/);
    expect(html).not.toMatch(/here's a suggestion|here&#x27;s a suggestion/);
  });

  it("falls back to '{series} #{number}' heading when title is null", () => {
    const data: NextUpView = {
      source: "series",
      target: issue({ title: null }),
    };
    const html = render({ data });
    expect(html).toMatch(/Invincible #2/);
  });

  it("anchors to the right edge in LTR mode", () => {
    const data: NextUpView = { source: "series", target: issue() };
    const html = render({ data, direction: "ltr" });
    expect(html).toMatch(/right-4/);
    expect(html).not.toMatch(/left-4/);
  });

  it("anchors to the left edge in RTL mode", () => {
    const data: NextUpView = { source: "series", target: issue() };
    const html = render({ data, direction: "rtl" });
    expect(html).toMatch(/left-4/);
    expect(html).not.toMatch(/right-4/);
  });

  it("translates off-screen + disables pointer events when closed", () => {
    const html = render({ open: false });
    expect(html).toMatch(/translate-x-\[calc/);
    expect(html).toMatch(/pointer-events-none/);
  });
});
