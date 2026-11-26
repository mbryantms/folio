/**
 * "Find event chapters…": which chapter breaks the picked proposals turn
 * into. One named break at each event's first issue and one unnamed break
 * where the list goes on, never where a chapter already starts.
 */
import { describe, expect, it } from "vitest";

import {
  chapterLabel,
  detectedBreaks,
} from "@/components/cbl/links/DetectChaptersDialog";
import type { ProposedChapter } from "@/lib/api/types";

function p(
  entry: string,
  position: number,
  next: string | null,
  over: Partial<ProposedChapter> = {},
): ProposedChapter {
  return {
    name: `[Marvel] Event ${entry} (WEB-CBRO)`,
    entry_id: entry,
    position,
    starts_at: "X #1",
    issues: 5,
    total: 6,
    end_position: position + 4,
    already_chapter: false,
    next_entry_id: next ?? undefined,
    next_already_chapter: false,
    ...over,
  } as ProposedChapter;
}

describe("detectedBreaks", () => {
  it("names the event's start and breaks where the list goes on", () => {
    const a = p("a", 10, "a-next");
    expect(detectedBreaks([a], new Set(["a"]))).toEqual([
      { entry_id: "a", label: chapterLabel(a) },
      { entry_id: "a-next", label: null },
    ]);
    expect(chapterLabel(a)).toBe("Event: Event a");
  });

  it("adds nothing for an unpicked proposal", () => {
    expect(detectedBreaks([p("a", 10, "n")], new Set())).toEqual([]);
  });

  it("skips places a chapter already starts", () => {
    const got = detectedBreaks(
      [p("a", 10, "n", { already_chapter: true, next_already_chapter: true })],
      new Set(["a"]),
    );
    expect(got).toEqual([]);
  });

  it("lets a back-to-back event's own break close the one before it", () => {
    // b starts at the entry right after a's stretch.
    const a = p("a", 10, "b");
    const b = p("b", 15, "after-b");
    const got = detectedBreaks([b, a], new Set(["a", "b"]));
    expect(got.map((x) => x.entry_id)).toEqual(["a", "b", "after-b"]);
    expect(got[1]!.label).toBe(chapterLabel(b));
    // Only a kept: the list goes on at b's first issue, unnamed.
    expect(detectedBreaks([a, b], new Set(["a"]))).toEqual([
      { entry_id: "a", label: chapterLabel(a) },
      { entry_id: "b", label: null },
    ]);
  });

  it("ends at the list's end without a closing break", () => {
    expect(detectedBreaks([p("a", 10, null)], new Set(["a"]))).toHaveLength(1);
  });
});
