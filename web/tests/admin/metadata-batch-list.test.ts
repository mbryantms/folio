import { describe, expect, it } from "vitest";

import { batchWhen } from "@/components/admin/metadata/ReviewTab";

describe("batchWhen", () => {
  it("labels a finished batch with its finish time", () => {
    const text = batchWhen({
      created_at: "2026-06-05T16:47:02Z",
      finished_at: "2026-06-05T16:49:10Z",
      in_flight: 0,
    });
    expect(text.startsWith("Finished ")).toBe(true);
  });

  it("labels an unfinished batch with its start time and what is left", () => {
    const text = batchWhen({
      created_at: "2026-06-05T16:47:02Z",
      finished_at: null,
      in_flight: 1200,
    });
    expect(text.startsWith("Started ")).toBe(true);
    expect(text.endsWith("· 1,200 still searching")).toBe(true);
  });

  it("omits the count when nothing is searching (e.g. waiting on quota)", () => {
    const text = batchWhen({
      created_at: "2026-06-05T16:47:02Z",
      finished_at: null,
      in_flight: 0,
    });
    expect(text.startsWith("Started ")).toBe(true);
    expect(text).not.toContain("still searching");
  });
});
