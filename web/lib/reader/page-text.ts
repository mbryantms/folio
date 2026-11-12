/**
 * Reader "page text" panel state + pure helpers (WP-4.8, audit AC-3).
 *
 * The panel exposes a page's OCR text to assistive tech. It reuses the
 * existing server OCR surface — `GET /me/issues/{id}/pages/{n}/text-regions`
 * for the detected bubbles, then `POST /me/issues/{id}/ocr` per bubble —
 * and only runs when the panel is opened, so no OCR work happens on page
 * load. The panel component itself is lazy-loaded; this module stays tiny
 * because the reader chrome, the skip links and the keymap all import it.
 * The box helpers the panel and the marker overlay share live in
 * `text-regions.ts`, which only lazy chunks import.
 */
import { create } from "zustand";

type PageTextPanelState = {
  open: boolean;
  setOpen: (open: boolean) => void;
  toggle: () => void;
  /** Opening line of the first text block (in reading order) the panel
   *  has read per page, keyed `${issueId}:${page}`, with the
   *  recognizer's language tag — the page image's `alt` (and its `lang`)
   *  is built from it (audit N-R4 / AC-3). Only pages the panel has
   *  fully read appear here; a page with no readable text is absent, so
   *  the image keeps its plain "Page N" label. */
  firstText: Record<string, PageFirstText>;
  setFirstText: (
    issueId: string,
    page: number,
    text: string | null,
    lang?: string | null,
  ) => void;
  /** Drop every stored line — the reader calls it on unmount so the map
   *  does not grow across issues for the life of the tab. */
  clearFirstText: () => void;
};

export type PageFirstText = {
  line: string;
  /** BCP-47 tag from {@link ocrLangTag}; absent for an unknown recognizer. */
  lang?: string;
};

function firstTextKey(issueId: string, page: number): string {
  return `${issueId}:${page}`;
}

/** Open/closed flag for the page-text panel. Kept out of the main reader
 *  store so the panel's wiring doesn't churn `store.ts`. */
export const usePageTextPanel = create<PageTextPanelState>((set) => ({
  open: false,
  setOpen: (open) => set({ open }),
  toggle: () => set((s) => ({ open: !s.open })),
  firstText: {},
  setFirstText: (issueId, page, text, lang) =>
    set((s) => {
      const key = firstTextKey(issueId, page);
      const line = text ? firstLineOf(text) : "";
      const tag = ocrLangTag(lang);
      const cur = s.firstText[key];
      if ((cur?.line ?? "") === line && cur?.lang === (line ? tag : undefined))
        return s;
      const next = { ...s.firstText };
      if (line) next[key] = tag ? { line, lang: tag } : { line };
      else delete next[key];
      return { firstText: next };
    }),
  clearFirstText: () =>
    set((s) => (Object.keys(s.firstText).length ? { firstText: {} } : s)),
}));

/** Longest `alt` the OCR'd first line may contribute. Alt text is read
 *  in one breath by a screen reader; a whole bubble can run to a
 *  paragraph, so the page label carries only the opening line. */
export const PAGE_ALT_MAX_CHARS = 120;

/** The first non-empty line of an OCR block, whitespace collapsed and
 *  cut to {@link PAGE_ALT_MAX_CHARS} with an ellipsis. Empty when the
 *  block is blank. */
export function firstLineOf(text: string): string {
  const line =
    text
      .split(/\r?\n/)
      .map((l) => l.replace(/\s+/g, " ").trim())
      .find((l) => l.length > 0) ?? "";
  if (line.length <= PAGE_ALT_MAX_CHARS) return line;
  return `${line.slice(0, PAGE_ALT_MAX_CHARS - 1).trimEnd()}…`;
}

/** `alt` for a page image: "Page N", or "Page N: <first line>" when the
 *  page-text panel has read text on that page. */
export function pageAltText(
  pageIndex: number,
  firstLine: string | null | undefined,
): string {
  const label = `Page ${pageIndex + 1}`;
  return firstLine ? `${label}: ${firstLine}` : label;
}

/** The page image's `alt` (and the `lang` its text is in) for `page`,
 *  live: plain "Page N" until the page-text panel has read the page, then
 *  prefixed with its first line. `lang` is only set when the quoted line
 *  came from a recognizer with a known language, so the screen reader
 *  switches voice for it the way the panel's rows do. */
export function usePageAltText(
  issueId: string,
  pageIndex: number,
): { alt: string; lang: string | undefined } {
  const first = usePageTextPanel(
    (s) => s.firstText[firstTextKey(issueId, pageIndex)],
  );
  return { alt: pageAltText(pageIndex, first?.line), lang: first?.lang };
}

/** BCP-47 tag for the recognizer the server resolved, so screen readers
 *  pick the right voice for manga text. */
export function ocrLangTag(
  lang: string | null | undefined,
): string | undefined {
  if (lang === "manga") return "ja";
  if (lang === "western") return "en";
  return undefined;
}
