import { describe, expect, it } from "vitest";

import {
  collectionStatus,
  collectionTooltip,
  formatSpecialsSuffix,
  ownedCompletionCount,
  ownedMainCount,
} from "@/lib/series-status";

describe("collectionStatus — main run vs specials", () => {
  it("does not let an annual complete a short run", () => {
    // 4 numbered issues + 1 annual against an expected 5: the old
    // helper compared issue_count (5) and called this complete.
    const series = {
      issue_count: 5,
      main_issue_count: 4,
      special_issue_count: 1,
      total_issues: 5,
    };
    expect(ownedMainCount(series)).toBe(4);
    expect(collectionStatus(series)).toBe("incomplete");
    expect(formatSpecialsSuffix(series)).toBe("+1 special");
    expect(collectionTooltip(series)).toBe("4 of 5 issues (+1 special)");
  });

  it("reports complete from the main run and lists extras", () => {
    const series = {
      issue_count: 7,
      main_issue_count: 4,
      special_issue_count: 3,
      total_issues: 4,
    };
    expect(collectionStatus(series)).toBe("complete");
    expect(formatSpecialsSuffix(series)).toBe("+3 specials");
    expect(collectionTooltip(series)).toBe(
      "Complete: 4 of 4 issues (+3 specials)",
    );
  });

  it("keeps >= semantics for over-collected numbered runs", () => {
    expect(
      collectionStatus({
        issue_count: 6,
        main_issue_count: 6,
        special_issue_count: 0,
        total_issues: 5,
      }),
    ).toBe("complete");
  });

  it("falls back to issue_count when the split is absent", () => {
    const legacy = { issue_count: 5, total_issues: 5 };
    expect(ownedMainCount(legacy)).toBe(5);
    expect(collectionStatus(legacy)).toBe("complete");
    expect(formatSpecialsSuffix(legacy)).toBeNull();
  });

  it("has no signal without total_issues", () => {
    expect(
      collectionStatus({
        issue_count: 3,
        main_issue_count: 3,
        special_issue_count: 0,
        total_issues: null,
      }),
    ).toBeNull();
  });
});

describe("collectionStatus — specials counted (B9)", () => {
  const annualled = {
    issue_count: 5,
    main_issue_count: 4,
    special_issue_count: 1,
    total_issues: 5,
  };

  it("lets an annual complete the run when the viewer counts specials", () => {
    const series = { ...annualled, specials_count_toward_completion: true };
    expect(ownedMainCount(series)).toBe(4);
    expect(ownedCompletionCount(series)).toBe(5);
    expect(collectionStatus(series)).toBe("complete");
    expect(formatSpecialsSuffix(series)).toBe("incl. 1 special");
    expect(collectionTooltip(series)).toBe(
      "Complete: 5 of 5 issues (incl. 1 special)",
    );
  });

  it("keeps the main-run rule when the flag is off or absent", () => {
    for (const series of [
      annualled,
      { ...annualled, specials_count_toward_completion: false },
    ]) {
      expect(ownedCompletionCount(series)).toBe(4);
      expect(collectionStatus(series)).toBe("incomplete");
    }
  });
});
