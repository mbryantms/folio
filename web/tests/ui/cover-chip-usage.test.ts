/**
 * Guard for the cover pill standard (WP-9.5,
 * docs/dev/card-corner-conventions.md "The pill"): a file that draws a
 * cover card puts text on the cover with `<CoverChip>` and the read mark
 * with `<ReadCheck>`, never a hand-built pill.
 *
 * Source-level on purpose: the mistakes this catches (a `<Badge>` nudged
 * into a corner, a span with `backdrop-blur`, a palette colour, a chip
 * restyled at the call site) all render fine in a DOM test.
 */
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

/** Every file that positions something on cover art. A new cover card
 *  goes here as well as in the conventions doc. */
const CARD_FILES = [
  "components/library/IssueCard.tsx",
  "components/library/SeriesCard.tsx",
  "components/library/ProgressIssueCard.tsx",
  "components/library/OnDeckCard.tsx",
  "components/library/SeriesRelatedSection.tsx",
  "components/library/CoverPagePickerDialog.tsx",
  "components/cbl/cbl-issue-card.tsx",
  "components/cbl/CblWindowCard.tsx",
  "components/upcoming/UpcomingCard.tsx",
  "components/upcoming/UpcomingCover.tsx",
  "components/upcoming/UpcomingStageBadge.tsx",
  "components/log/ActivityReportPage.tsx",
  "components/markers/MarkersList.tsx",
];

const read = (file: string) =>
  readFileSync(resolve(__dirname, "../..", file), "utf8");

/** Class strings that position an element (`absolute`) in this source. */
function positionedClassStrings(source: string): string[] {
  return [...source.matchAll(/"([^"\n]*\babsolute\b[^"\n]*)"/g)].map(
    (m) => m[1]!,
  );
}

/** A positioned element styled as a text pill by hand: it leans on the
 *  blur, or carries its own rounded, padded, sized-text box. */
function isHandBuiltPill(classes: string): boolean {
  if (/\bbackdrop-blur/.test(classes)) return true;
  return (
    /\brounded/.test(classes) &&
    /\bpx-/.test(classes) &&
    /\btext-(xs|sm|\[)/.test(classes)
  );
}

describe.each(CARD_FILES)("%s", (file) => {
  const source = read(file);

  it("positions no hand-built pill on the cover", () => {
    expect(positionedClassStrings(source).filter(isHandBuiltPill)).toEqual([]);
  });

  it("puts no <Badge> on the cover", () => {
    // A Badge opening tag whose own props include an absolute position.
    const badges = [...source.matchAll(/<Badge\b[^>]*>/g)].map((m) => m[0]);
    expect(badges.filter((tag) => /\babsolute\b/.test(tag))).toEqual([]);
  });

  it("uses theme tokens, not palette colours", () => {
    const palette =
      /\b(bg|text|ring|border)-(amber|rose|sky|yellow|emerald|red|blue|green|orange|purple|pink|slate|zinc|neutral)-\d{2,3}\b/g;
    expect(source.match(palette) ?? []).toEqual([]);
  });

  it("positions a CoverChip without restyling it", () => {
    const chips = [...source.matchAll(/<CoverChip\b[\s\S]*?>/g)].map(
      (m) => m[0],
    );
    const restyle =
      /\b(bg|text|border|ring|shadow|rounded|backdrop|font|px|py|h)-/;
    for (const tag of chips) {
      const classes = /className="([^"]*)"/.exec(tag)?.[1] ?? "";
      expect(restyle.test(classes), `${file}: ${classes}`).toBe(false);
    }
  });
});

describe("the guard itself", () => {
  it("recognises the pills it replaced", () => {
    for (const old of [
      "bg-background/80 absolute top-2 right-2 backdrop-blur",
      "bg-background/85 text-foreground absolute bottom-2 left-2 rounded px-1.5 py-0.5 text-[11px] leading-4 font-medium backdrop-blur",
      "absolute top-2 left-2 inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-[10px] font-medium uppercase",
    ]) {
      expect(isHandBuiltPill(old), old).toBe(true);
    }
  });

  it("leaves progress bars and icon badges alone", () => {
    for (const fine of [
      "bg-background/70 absolute inset-x-0 bottom-0 h-1.5 overflow-hidden rounded-b-md",
      "absolute bottom-2.5 left-2 inline-flex size-5 items-center justify-center rounded-full shadow-sm ring-1",
      "absolute top-2 right-2 max-w-[calc(100%-1rem)]",
    ]) {
      expect(isHandBuiltPill(fine), fine).toBe(false);
    }
  });

  it("covers every card the conventions doc lists", () => {
    const doc = readFileSync(
      resolve(__dirname, "../../../docs/dev/card-corner-conventions.md"),
      "utf8",
    );
    const list = /The cover cards across Folio \(([^)]*)\)/.exec(doc);
    const named = [...list![1]!.matchAll(/`(\w+)`/g)].map((m) => m[1]!);
    for (const card of named) {
      const hit = CARD_FILES.some((f) =>
        new RegExp(`export (function|const) ${card}\\b`).test(read(f)),
      );
      expect(hit, `${card} is drawn by a guarded file`).toBe(true);
    }
  });
});
