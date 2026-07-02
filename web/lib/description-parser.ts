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

  // ComicVine (and many providers) return descriptions as raw HTML:
  //   "<p><i>Rick Grimes...</i></p><h4>List of covers...</h4><table>...</table>"
  // The marker-based parser below expects ComicTagger-style `*Title:*`
  // structure, so HTML payloads fell through and rendered as a wall of
  // visible markup. Detect HTML first and convert it into the same
  // ParsedDescription shape — intro prose + parsed tables — so the
  // existing render pipeline doesn't need to change.
  if (looksLikeHtml(trimmed)) {
    return parseHtmlDescription(trimmed);
  }

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
  const firstTitle = firstTableMatch?.[1];
  if (
    !firstTableMatch ||
    firstTableMatch.index === undefined ||
    firstTitle === undefined
  ) {
    return fallback;
  }

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
      title: firstTitle.trim().replace(/:$/, "").trim(),
      isTableHeader: true,
    },
  ];

  const restRe = /\*([^*\n]+?)\*/g;
  restRe.lastIndex = firstTableMatch.index + firstTableMatch[0].length;
  let m: RegExpExecArray | null;
  while ((m = restRe.exec(trimmed)) !== null) {
    const inner = m[1]?.trim();
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
    if (marker === undefined) continue;
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
    // Group 1 spans the whole match (the lookahead is zero-width), so the
    // full-match text is an equivalent stand-in for the capture.
    matches.push({
      index: pm.index,
      length: pm[0].length,
      prefix: pm[1] ?? pm[0],
    });
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
    if (c === undefined) continue;
    const ordDigits = c.prefix.match(/^(\d+)(st|nd|rd|th)/)?.[1];
    if (ordDigits === undefined || ordDigits.length < 2) continue;
    if (Number(ordDigits) <= 10) continue;
    c.index += 1;
    c.length -= 1;
    c.prefix = c.prefix.slice(1);
  }

  type RawRow = { prefix: string; middle: string; sidebar: string };
  const rawRows: RawRow[] = [];
  for (let i = 0; i < matches.length; i++) {
    const cur = matches[i];
    if (cur === undefined) continue;
    const nextMatch = matches[i + 1];
    const start = cur.index + cur.length;
    const end = nextMatch !== undefined ? nextMatch.index : rest.length;
    const cellBody = rest.slice(start, end).trim();
    const sbMatch = cellBody.match(/(\d+)\s*$/);
    const sidebar = sbMatch?.[1];
    if (!sbMatch || sbMatch.index === undefined || sidebar === undefined) {
      continue;
    }
    const middle = cellBody.slice(0, sbMatch.index).trim();
    if (!middle) continue;
    rawRows.push({
      prefix: cur.prefix.replace(/\s+/g, " ").trim(),
      middle,
      sidebar,
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
  const [first, ...restRows] = rows;
  if (first === undefined || restRows.length === 0) return null;
  let commonReversed = [...first].reverse().join("");
  for (const row of restRows) {
    const r = [...row].reverse().join("");
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
    if (ch === undefined) break;
    const atStart = i === 0;
    const prev = suffix[i - 1];
    const prevIsSpace = !atStart && prev !== undefined && /\s/.test(prev);
    if (/[A-Z]/.test(ch) && (atStart || prevIsSpace)) break;
    i++;
  }
  if (i >= suffix.length) return null;
  const trimmed = suffix.slice(i).trim();
  if (trimmed.length < 3) return null;
  return trimmed;
}

/** Light HTML detection — looks for an opening tag in `<lowercase-letter`
 *  shape. ComicVine descriptions hit this; ComicTagger markdown-ish
 *  payloads don't. */
function looksLikeHtml(text: string): boolean {
  return /<(p|i|b|em|strong|h[1-6]|ul|ol|li|table|br|div|span)[\s/>]/i.test(
    text,
  );
}

/**
 * Convert an HTML-shaped description into the `ParsedDescription` shape.
 * Extracts every `<table>` block (parsed with header + rows preserved)
 * and treats `<h1>..<h6>` text as section titles. The remainder — prose
 * paragraphs and inline emphasis — is flattened to plain text for the
 * intro / section bodies. The result renders through the existing
 * `<Description>` component verbatim.
 *
 * Implementation note: we don't sanitize-then-`dangerouslySetInnerHTML`
 * because the existing UX is built around plain-text-plus-table; adding
 * a separate HTML-renderer would mean two divergent code paths. The
 * structured outcome (intro + tables) matches what users expect from
 * provider summaries.
 */
function parseHtmlDescription(html: string): ParsedDescription {
  const tables: DescriptionTable[] = [];
  const sections: DescriptionSection[] = [];

  // Pull every <table>...</table> block out first so we can render it
  // structurally. Case-insensitive, multiline, non-greedy.
  let remainder = html;
  const tableRe = /<table\b[^>]*>([\s\S]*?)<\/table>/gi;
  let match: RegExpExecArray | null;
  while ((match = tableRe.exec(html)) !== null) {
    const t = parseHtmlTable(match[1] ?? "");
    if (t) tables.push(t);
  }
  remainder = remainder.replace(tableRe, "\n");

  // Sectioning: `<h1>..<h6>` titles become DescriptionSection entries
  // with everything up to the next heading as their body. Pre-section
  // text becomes the intro.
  const headingRe = /<h([1-6])\b[^>]*>([\s\S]*?)<\/h\1>/gi;
  type HeadingMatch = { index: number; length: number; title: string };
  const headings: HeadingMatch[] = [];
  while ((match = headingRe.exec(remainder)) !== null) {
    headings.push({
      index: match.index,
      length: match[0].length,
      title: stripInlineHtml(match[2] ?? "").trim(),
    });
  }

  let intro: string;
  const firstHeading = headings[0];
  if (firstHeading === undefined) {
    intro = htmlToPlainText(remainder);
  } else {
    intro = htmlToPlainText(remainder.slice(0, firstHeading.index));
    for (let i = 0; i < headings.length; i++) {
      const h = headings[i];
      if (h === undefined) continue;
      const nextHeading = headings[i + 1];
      const bodyStart = h.index + h.length;
      const bodyEnd =
        nextHeading !== undefined ? nextHeading.index : remainder.length;
      const body = htmlToPlainText(remainder.slice(bodyStart, bodyEnd));
      if (h.title && body) sections.push({ title: h.title, text: body });
    }
  }

  const hasStructuredContent = tables.length > 0 || sections.length > 0;
  return { intro, tables, sections, hasStructuredContent };
}

/** Parse the inner body of a `<table>` element into title-less rows +
 *  columns. The first `<tr>` whose cells are `<th>` becomes the header;
 *  remaining `<tr>` rows become the body. Picks up `data-*` attributes
 *  optionally on the wrapping element (currently unused but tolerated). */
function parseHtmlTable(inner: string): DescriptionTable | null {
  const rowRe = /<tr\b[^>]*>([\s\S]*?)<\/tr>/gi;
  const rows: string[][] = [];
  let header: string[] | null = null;
  let m: RegExpExecArray | null;
  while ((m = rowRe.exec(inner)) !== null) {
    const rowHtml = m[1] ?? "";
    const isHeader = /<th\b/i.test(rowHtml);
    const cellRe = isHeader
      ? /<th\b[^>]*>([\s\S]*?)<\/th>/gi
      : /<td\b[^>]*>([\s\S]*?)<\/td>/gi;
    const cells: string[] = [];
    let cm: RegExpExecArray | null;
    while ((cm = cellRe.exec(rowHtml)) !== null) {
      cells.push(stripInlineHtml(cm[1] ?? "").trim());
    }
    if (cells.length === 0) continue;
    if (isHeader && !header) {
      header = cells;
    } else {
      rows.push(cells);
    }
  }
  if (rows.length === 0) return null;
  // Tables without explicit headers get a synthetic header row built
  // from the longest body row — keeps the existing renderer's column
  // count stable.
  if (!header) {
    const widest = rows.reduce((a, r) => Math.max(a, r.length), 0);
    header = Array.from({ length: widest }, (_, i) => `Col ${i + 1}`);
  }
  return { title: "List of covers", columns: header, rows };
}

/** Strip inline HTML tags while preserving text. Used inside table
 *  cells and headings where block-level structure was already stripped. */
function stripInlineHtml(html: string): string {
  return decodeHtmlEntities(html.replace(/<[^>]+>/g, "")).trim();
}

/** Flatten block-and-inline HTML into a paragraph-friendly plain string.
 *  `<br>` and `</p>` become a single space (`<Description>` already
 *  controls paragraph spacing via CSS); other tags are dropped. Multiple
 *  spaces collapse to one so the prose reads cleanly even with the
 *  flattening. */
function htmlToPlainText(html: string): string {
  const withBreaks = html
    .replace(/<br\s*\/?>/gi, " ")
    .replace(/<\/(p|div|li|h[1-6])>/gi, " ")
    .replace(/<[^>]+>/g, "");
  return decodeHtmlEntities(withBreaks).replace(/\s+/g, " ").trim();
}

/** Decode the five XML predefined entities plus numeric character
 *  references. Keeps the conversion self-contained — no DOMParser
 *  needed (SSR-safe, vitest-safe). */
function decodeHtmlEntities(text: string): string {
  return text
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&quot;/g, '"')
    .replace(/&apos;/g, "'")
    .replace(/&#x([0-9a-fA-F]+);/g, (_, hex) =>
      String.fromCodePoint(parseInt(hex, 16)),
    )
    .replace(/&#(\d+);/g, (_, dec) => String.fromCodePoint(parseInt(dec, 10)))
    .replace(/&amp;/g, "&");
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
