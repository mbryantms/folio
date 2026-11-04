import { describe, expect, it } from "vitest";

import { seriesFinishProgress } from "@/lib/reader/finish-progress";

describe("seriesFinishProgress", () => {
  it("adds this issue when the read counts", () => {
    expect(
      seriesFinishProgress("Saga", { finished: 3, total: 10 }, true),
    ).toEqual({
      scope: "series",
      seriesName: "Saga",
      total: 10,
      before: 3,
      after: 4,
    });
  });

  it("measures a reading list when given the list scope", () => {
    expect(
      seriesFinishProgress(
        "Civil War",
        { finished: 7, total: 30 },
        true,
        "list",
      ),
    ).toMatchObject({
      scope: "list",
      seriesName: "Civil War",
      before: 7,
      after: 8,
    });
  });

  it("moves nothing on a re-read, incognito or peek", () => {
    const p = seriesFinishProgress("Saga", { finished: 3, total: 10 }, false);
    expect(p?.before).toBe(3);
    expect(p?.after).toBe(3);
  });

  it("never passes the total, and clamps bad counts", () => {
    expect(
      seriesFinishProgress("Saga", { finished: 10, total: 10 }, true)?.after,
    ).toBe(10);
    expect(
      seriesFinishProgress("Saga", { finished: -2, total: 4 }, true),
    ).toMatchObject({ before: 0, after: 1 });
  });

  it("is unknown without a snapshot or with an empty series", () => {
    expect(seriesFinishProgress("Saga", null, true)).toBeNull();
    expect(
      seriesFinishProgress("Saga", { finished: 0, total: 0 }, true),
    ).toBeNull();
  });
});
