/**
 * Linked reading lists (PR 2b): how the list page lays a master list out
 * as chapters with event bands, and where its entries query starts.
 *
 * Pure functions so the folding rules are unit-testable
 * (`web/tests/cbl/linked-plan.test.ts`).
 *
 * - A chapter the reader finished folds by default (only its header and
 *   its events show), so the page opens at the chapter they are in.
 * - The entries query starts at the first unfolded chapter
 *   (`from_position`), so a long list never walks every earlier page.
 * - With a search active, folding is off: chapters appear only where
 *   matching entries are.
 */
import type {
  ReadingListChapterView,
  ReadingListLinkView,
} from "@/lib/api/types";

export type PlanItem<E> =
  | { kind: "entry"; entry: E; position: number }
  | { kind: "gap"; key: string; count: number; position: number };

export type ChapterBlock<E> = {
  /** `null` when the list has no chapters (no links yet). */
  chapter: ReadingListChapterView | null;
  folded: boolean;
  items: PlanItem<E>[];
  /** Events read right after this chapter; only once it is fully
   *  loaded, so a band never shows ahead of entries still to come. */
  events: ReadingListLinkView[];
};

/** Everything the reader owns in the chapter is read. */
export function isChapterDone(c: ReadingListChapterView): boolean {
  return c.in_library > 0 && c.read >= c.in_library;
}

/**
 * The chapters drawn folded. Finished chapters fold unless `showRead`;
 * `overrides[index]` (the reader clicked the header) wins either way.
 * `doneAtLoad` freezes which chapters count as finished to when the page
 * opened, so marking the last issue of a chapter read doesn't fold it out
 * from under the reader.
 */
export function foldedChapters(
  chapters: readonly ReadingListChapterView[],
  overrides: Readonly<Record<number, boolean>>,
  showRead: boolean,
  doneAtLoad?: ReadonlySet<number>,
): Set<number> {
  const out = new Set<number>();
  for (const c of chapters) {
    const o = overrides[c.index];
    const done = doneAtLoad ? doneAtLoad.has(c.index) : isChapterDone(c);
    const folded = o ?? (!showRead && done);
    if (folded) out.add(c.index);
  }
  return out;
}

/**
 * Where the entries query starts: the first unfolded chapter's first
 * position, or `undefined` (the top) when that is chapter 0 or there are
 * no chapters. When every chapter is folded, the last one's start (so
 * nothing earlier loads).
 */
export function seekStart(
  chapters: readonly ReadingListChapterView[],
  folded: ReadonlySet<number>,
): number | undefined {
  if (chapters.length === 0) return undefined;
  const first = chapters.find((c) => !folded.has(c.index));
  const start = (first ?? chapters[chapters.length - 1]!).start_position;
  return start > 0 ? start : undefined;
}

/**
 * Group the loaded items into chapter blocks.
 *
 * @param exhausted no more pages to load (the last chapter is complete).
 * @param foldingActive false while a search is active.
 */
export function buildChapterBlocks<E>(
  items: readonly PlanItem<E>[],
  chapters: readonly ReadingListChapterView[],
  events: readonly ReadingListLinkView[],
  folded: ReadonlySet<number>,
  foldingActive: boolean,
  exhausted: boolean,
): ChapterBlock<E>[] {
  if (chapters.length === 0) {
    return [{ chapter: null, folded: false, items: [...items], events: [] }];
  }
  const byId = new Map(events.map((e) => [e.id, e]));
  const eventsOf = (c: ReadingListChapterView) =>
    c.event_link_ids
      .map((id) => byId.get(id))
      .filter((e): e is ReadingListLinkView => e != null);
  const blocks: ChapterBlock<E>[] = [];
  for (const c of chapters) {
    if (foldingActive && folded.has(c.index)) {
      blocks.push({ chapter: c, folded: true, items: [], events: eventsOf(c) });
      continue;
    }
    const mine = items.filter(
      (i) => i.position >= c.start_position && i.position <= c.end_position,
    );
    const complete =
      exhausted || items.some((i) => i.position > c.end_position);
    if (!foldingActive && mine.length === 0) {
      if (!complete) break;
      continue;
    }
    blocks.push({
      chapter: c,
      folded: false,
      items: mine,
      events: complete && (foldingActive || mine.length > 0) ? eventsOf(c) : [],
    });
    if (!complete) break;
  }
  return blocks;
}
