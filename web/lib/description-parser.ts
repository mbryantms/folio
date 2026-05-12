/**
 * Description parser for ComicInfo `summary` fields that contain inline
 * Markdown-ish tables (variant cover lists, etc.). The source text loses
 * its newlines on the way through ComicInfo, so a typical payload looks
 * like:
 *
 *   "Some prose. *List of covers and their creators:* Cover | Name | … |
 *    --------- Reg | Regular Cover | Chip Zdarsky | 1 | …"
 *
 * The parser splits the text into:
 * - `intro`: the prose before any structured marker
 * - `tables`: any "*Heading:*"-introduced pipe tables
 * - `sections`: any "*Heading*" (no colon) free-text sections, e.g. "*Notes*"
 *
 * If no clear table marker is found, the whole text comes back as `intro`
 * and `hasStructuredContent` is false — callers should fall back to a
 * plain paragraph render.
 */

export type DescriptionTable = {
  title: string;
  columns: string[];
  rows: string[][];
};

export type DescriptionSection = {
  title: string;
  text: string;
};

export type ParsedDescription = {
  intro: string;
  tables: DescriptionTable[];
  sections: DescriptionSection[];
  hasStructuredContent: boolean;
};

const EMPTY: ParsedDescription = {
  intro: "",
  tables: [],
  sections: [],
  hasStructuredContent: false,
};

export function parseDescription(
  text: string | null | undefined,
): ParsedDescription {
  if (!text) return EMPTY;
  const trimmed = text.trim();
  if (!trimmed) return EMPTY;

  const fallback: ParsedDescription = {
    intro: trimmed,
    tables: [],
    sections: [],
    hasStructuredContent: false,
  };

  // We treat any "*Title:*" marker as the structure boundary, even when the
  // tail isn't a clean pipe table. ComicInfo `summary` payloads sometimes
  // arrive with all separators stripped (column-soup like
  // "CoverNameCreatorsSidebar LocationRegRegular Cover..."), and those
  // still need to be hidden behind the collapse so the prose stays
  // readable. We try to parse a real table first and fall back to raw
  // labeled text when we can't.
  const tableHeadingRe = /\*([^*\n]+?):\*/;
  const firstTableMatch = trimmed.match(tableHeadingRe);
  if (!firstTableMatch || firstTableMatch.index === undefined) return fallback;

  const intro = trimmed.slice(0, firstTableMatch.index).trim();

  type Marker = {
    index: number;
    length: number;
    title: string;
    isTableHeader: boolean;
  };
  const markers: Marker[] = [
    {
      index: firstTableMatch.index,
      length: firstTableMatch[0].length,
      title: firstTableMatch[1].trim().replace(/:$/, "").trim(),
      isTableHeader: true,
    },
  ];

  const restRe = /\*([^*\n]+?)\*/g;
  restRe.lastIndex = firstTableMatch.index + firstTableMatch[0].length;
  let m: RegExpExecArray | null;
  while ((m = restRe.exec(trimmed)) !== null) {
    const inner = m[1].trim();
    if (!inner) continue;
    markers.push({
      index: m.index,
      length: m[0].length,
      title: inner.replace(/:$/, "").trim(),
      isTableHeader: inner.endsWith(":"),
    });
  }

  const tables: DescriptionTable[] = [];
  const sections: DescriptionSection[] = [];

  for (let i = 0; i < markers.length; i++) {
    const marker = markers[i];
    const next = markers[i + 1];
    const start = marker.index + marker.length;
    const end = next ? next.index : trimmed.length;
    const body = trimmed.slice(start, end).trim();
    if (!body) continue;

    if (marker.isTableHeader) {
      const table =
        parseTable(marker.title, body) ??
        parseSmushedCoverList(marker.title, body);
      if (table) {
        tables.push(table);
        continue;
      }
    }
    sections.push({ title: marker.title, text: body });
  }

  if (tables.length === 0 && sections.length === 0) return fallback;

  return { intro, tables, sections, hasStructuredContent: true };
}

/**
 * Recover a cover list from "smushed" descriptions where ComicInfo
 * preserved the column-name header and cover-type prefixes but stripped
 * every cell separator. Example payload:
 *
 *   "CoverNameCreatorsSidebar LocationRegRegular CoverFiona Staples1Var
 *    C2E2 ... copies)Fiona Staples62nd PrintSecond Printing CoverFiona
 *    Staples8 ..."
 *
 * Strategy:
 *  1. Detect and strip the canonical "Cover Name Creator(s) Sidebar
 *     Location" header.
 *  2. Split the rest into row segments at recognized cover-type prefixes
 *     ("Reg", "Var", "RI", "RE", "<N>th Print[ing]"), anchored to a
 *     following capital-letter cell start so we don't false-match inside
 *     "Regular", "Variant", or "Retailer".
 *  3. Within each row, the trailing digits are the sidebar location and
 *     the rest is "<Name><Creator>" with no separator.
 *  4. Find the longest common reversed-prefix across all rows' middles
 *     and use it as the creator. If we can't recover a clean creator
 *     suffix, emit 3 columns ("Cover", "Name & Creator(s)", "Sidebar")
 *     so the data is still legible.
 */
function parseSmushedCoverList(
  title: string,
  rawBody: string,
): DescriptionTable | null {
  const body = rawBody.replace(/\s+/g, " ").trim();

  const headerRe =
    /^Cover\s*Name\s*Creator(?:\(s\)|s)?\s*Sidebar\s*Location\s*/;
  const headerMatch = body.match(headerRe);
  if (!headerMatch) return null;
  const rest = body.slice(headerMatch[0].length).trim();
  if (!rest) return null;

  const PREFIX_RE =
    /(Reg|Var|RI|RE|\d+(?:st|nd|rd|th)\s*Print(?:ing)?)(?=\s*[A-Z])/g;
  const matches: { index: number; length: number; prefix: string }[] = [];
  let pm: RegExpExecArray | null;
  while ((pm = PREFIX_RE.exec(rest)) !== null) {
    matches.push({ index: pm.index, length: pm[0].length, prefix: pm[1] });
  }
  if (matches.length === 0) return null;

  // The greedy `\d+` happily eats the previous row's sidebar digit when
  // rows abut with no separator (e.g. "...Fiona Staples55th Print" gets
  // captured as "55th" instead of sidebar=5 + ordinal=5th). If a non-first
  // ordinal match has an unreasonable value (>10), shift its start forward
  // by one so the leading digit returns to the previous row's tail where
  // it'll be picked up as a sidebar number. This handles the common
  // single-digit sidebar / single-digit ordinal pairing; genuinely big
  // ordinals (10th+ printings preceded by multi-digit sidebars) remain a
  // future refinement.
  for (let i = 1; i < matches.length; i++) {
    const c = matches[i];
    const ord = c.prefix.match(/^(\d+)(st|nd|rd|th)/);
    if (!ord || ord[1].length < 2) continue;
    if (Number(ord[1]) <= 10) continue;
    c.index += 1;
    c.length -= 1;
    c.prefix = c.prefix.slice(1);
  }

  type RawRow = { prefix: string; middle: string; sidebar: string };
  const rawRows: RawRow[] = [];
  for (let i = 0; i < matches.length; i++) {
    const start = matches[i].index + matches[i].length;
    const end = i + 1 < matches.length ? matches[i + 1].index : rest.length;
    const cellBody = rest.slice(start, end).trim();
    const sbMatch = cellBody.match(/(\d+)\s*$/);
    if (!sbMatch || sbMatch.index === undefined) continue;
    const middle = cellBody.slice(0, sbMatch.index).trim();
    if (!middle) continue;
    rawRows.push({
      prefix: matches[i].prefix.replace(/\s+/g, " ").trim(),
      middle,
      sidebar: sbMatch[1],
    });
  }
  if (rawRows.length === 0) return null;

  const creator = findCommonCreator(rawRows.map((r) => r.middle));

  if (creator) {
    return {
      title,
      columns: ["Cover", "Name", "Creator(s)", "Sidebar"],
      rows: rawRows.map((r) => {
        const name = r.middle.endsWith(creator)
          ? r.middle.slice(0, r.middle.length - creator.length).trim()
          : r.middle;
        return [r.prefix, name, creator, r.sidebar];
      }),
    };
  }

  return {
    title,
    columns: ["Cover", "Name & Creator(s)", "Sidebar"],
    rows: rawRows.map((r) => [r.prefix, r.middle, r.sidebar]),
  };
}

/**
 * Longest-common-suffix across the row middles, trimmed to start at a
 * clean word boundary (capital letter at string start or preceded by
 * whitespace). Returns null if the common tail is shorter than 3 chars
 * or doesn't have a usable boundary — in which case the caller falls
 * back to the 3-column "Name & Creator(s)" layout.
 */
function findCommonCreator(rows: string[]): string | null {
  if (rows.length < 2) return null;
  let commonReversed = [...rows[0]].reverse().join("");
  for (let i = 1; i < rows.length; i++) {
    const r = [...rows[i]].reverse().join("");
    let j = 0;
    while (
      j < commonReversed.length &&
      j < r.length &&
      commonReversed[j] === r[j]
    ) {
      j++;
    }
    commonReversed = commonReversed.slice(0, j);
    if (!commonReversed) return null;
  }
  const suffix = [...commonReversed].reverse().join("");
  // Walk forward to the first capital letter that sits at the start of
  // a word (start-of-string or preceded by whitespace) — that's where
  // the creator name actually begins.
  let i = 0;
  while (i < suffix.length) {
    const ch = suffix[i];
    const atStart = i === 0;
    const prevIsSpace = !atStart && /\s/.test(suffix[i - 1]);
    if (/[A-Z]/.test(ch) && (atStart || prevIsSpace)) break;
    i++;
  }
  if (i >= suffix.length) return null;
  const trimmed = suffix.slice(i).trim();
  if (trimmed.length < 3) return null;
  return trimmed;
}

function parseTable(title: string, body: string): DescriptionTable | null {
  const dividerIdx = body.search(/-{3,}/);
  if (dividerIdx <= 0) return null;

  const headerPart = body.slice(0, dividerIdx).trim();
  const dividerRun = body.slice(dividerIdx).match(/^-+\s*/);
  const bodyStart = dividerIdx + (dividerRun ? dividerRun[0].length : 0);
  const bodyPart = body.slice(bodyStart).trim();

  const columns = headerPart
    .split("|")
    .map((c) => c.trim())
    .filter((c) => c.length > 0);
  if (columns.length < 2) return null;

  const cells = bodyPart.split("|").map((c) => c.trim());
  while (cells.length > 0 && cells[cells.length - 1] === "") cells.pop();
  if (cells.length === 0) return null;

  const rows: string[][] = [];
  for (let i = 0; i < cells.length; i += columns.length) {
    const row = cells.slice(i, i + columns.length);
    while (row.length < columns.length) row.push("");
    if (row.some((c) => c.length > 0)) rows.push(row);
  }
  if (rows.length === 0) return null;

  return { title, columns, rows };
}
