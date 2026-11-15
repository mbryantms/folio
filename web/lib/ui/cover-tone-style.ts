/**
 * A cover's colour pair, decoded and turned into inline styles.
 *
 * Not a client module on purpose: server components (the series and
 * issue pages' header covers) call these to put the colour into the
 * markup they render. The cache and the canvas sampling are in
 * `cover-tone.ts`, which is client-only.
 *
 * The pair is twelve lowercase hex digits, the average colour then the
 * most vibrant: what the server's `metadata::cover_tone` writes and what
 * the device cache stores. Anything else decodes to `null`, so a value
 * can go into a `style` attribute safely.
 */
export type Tone = { average: string; vibrant: string };

const toRgb = (h: string) => {
  const n = parseInt(h, 16);
  return `${(n >> 16) & 255} ${(n >> 8) & 255} ${n & 255}`;
};

/** The pair as two `"r g b"` triplets, or `null` for anything that is not
 *  twelve lowercase hex digits. */
export function decodeTone(v: string | null | undefined): Tone | null {
  if (!v || !/^[0-9a-f]{12}$/.test(v)) return null;
  return { average: toRgb(v.slice(0, 6)), vibrant: toRgb(v.slice(6)) };
}

/**
 * The card's hover tint as an inline style, from the colour pair the
 * server sent with the cover (`cover_tone`, twelve hex digits). Spread it
 * on the card's `[data-card]` element so the tint is in the server-rendered
 * markup: a device that has never loaded the cover still washes and glows
 * in its colour. `undefined` when there is no pair (the per-device cache
 * then sets the same property from `useCoverTone`).
 */
export function coverTintStyle(
  tone: string | null | undefined,
): React.CSSProperties | undefined {
  const t = decodeTone(tone);
  return t
    ? ({ "--cover-tint": `rgb(${t.vibrant})` } as React.CSSProperties)
    : undefined;
}

/** The cover box's placeholder colour as an inline style, from the same
 *  pair: the cover's average colour, there on first paint. */
export function coverPlaceholderStyle(
  tone: string | null | undefined,
): React.CSSProperties | undefined {
  const t = decodeTone(tone);
  return t ? { backgroundColor: `rgb(${t.average})` } : undefined;
}
