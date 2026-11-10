/**
 * `issueLabel` / `issueCardTitle` (lib/format.ts) — the one display label
 * for an issue. Regression: an untitled issue read "#3 #3" in cards,
 * toasts and dialogs because several call sites appended the number to
 * a heading that was already the number.
 */
import { describe, expect, it } from "vitest";

import { issueCardTitle, issueLabel } from "@/lib/format";

describe("issueLabel", () => {
  it("uses a real title", () => {
    expect(issueLabel({ title: "Eight Is Enough", number: "2" })).toBe(
      "Eight Is Enough",
    );
  });

  it("appends the number to a title only when asked", () => {
    expect(
      issueLabel(
        { title: "Eight Is Enough", number: "2" },
        { withNumber: true },
      ),
    ).toBe("Eight Is Enough #2");
    expect(issueLabel({ title: "One-shot" }, { withNumber: true })).toBe(
      "One-shot",
    );
  });

  it("keeps a special's kind when the caller passes its number text", () => {
    expect(
      issueLabel(
        { title: "Home Again", number: "1" },
        { withNumber: true, numberLabel: "Annual #1" },
      ),
    ).toBe("Home Again Annual #1");
    expect(issueLabel({ number: "1" }, { numberLabel: "Annual #1" })).toBe(
      "Annual #1",
    );
    expect(
      issueLabel(
        { number: "1", series_name: "Saga" },
        { numberLabel: "Annual #1" },
      ),
    ).toBe("Saga Annual #1");
    // No number: the kind alone is not appended to a title.
    expect(
      issueLabel(
        { title: "Home Again" },
        { withNumber: true, numberLabel: "One-shot" },
      ),
    ).toBe("Home Again");
  });

  it("treats a blank title as no title", () => {
    expect(issueLabel({ title: "   ", number: "3" })).toBe("#3");
  });

  it("names an untitled issue by series and number", () => {
    expect(issueLabel({ number: "3", series_name: "Saga" })).toBe("Saga #3");
    expect(issueLabel({ number: "3" }, { seriesName: "Saga" })).toBe("Saga #3");
  });

  it("prefers the explicit series name over the issue's own", () => {
    expect(
      issueLabel({ number: "3", series_name: "Old" }, { seriesName: "New" }),
    ).toBe("New #3");
  });

  it("falls back to the bare number without a series name", () => {
    expect(issueLabel({ title: null, number: "3" })).toBe("#3");
  });

  it("never repeats the number for an untitled issue", () => {
    expect(issueLabel({ number: "3" }, { withNumber: true })).toBe("#3");
    expect(
      issueLabel({ number: "3", series_name: "Saga" }, { withNumber: true }),
    ).toBe("Saga #3");
  });

  it("falls back to the series name, then Untitled", () => {
    expect(issueLabel({ series_name: "Saga" })).toBe("Saga");
    expect(issueLabel({})).toBe("Untitled");
  });
});

describe("issueCardTitle", () => {
  it("is null when the label would only repeat the card's number", () => {
    expect(issueCardTitle({ number: "3" })).toBeNull();
  });

  it("keeps a title, a series label and the no-number fallback", () => {
    expect(issueCardTitle({ title: "Hello", number: "3" })).toBe("Hello");
    expect(issueCardTitle({ number: "3" }, "Saga")).toBe("Saga #3");
    expect(issueCardTitle({})).toBe("Untitled");
  });
});
