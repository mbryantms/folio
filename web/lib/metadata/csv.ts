/**
 * ComicInfo-shaped flat CSV fields (`writer`, `characters`, `genre`, …),
 * split the way the server does. Mirrors
 * `server::library::scanner::metadata_rollup::split_csv` exactly — the
 * issue page renders these columns straight from the issue row, so any
 * drift between the two shows up as chips that disagree with the
 * creator / character pages built from the junction tables.
 *
 * Rules:
 * - `;` anywhere → it is the sole separator (the composer's escape for
 *   names that contain commas, `"Capes, Inc."`); otherwise split on `,`.
 * - A comma piece that is only a generational suffix (`Jr` / `Sr` / `II`
 *   / `III` / `IV`, dotted or not) belongs to the name before it:
 *   `"José Marzán, Jr."` → `"José Marzán Jr."`, spelled the provider way.
 * - Dedupe is case-insensitive, first casing wins; empty pieces drop.
 */

/** Canonical spelling of a bare generational suffix, or `null`. `V` and
 *  single letters are initials, not suffixes. */
export function generationalSuffix(piece: string): string | null {
  switch (piece.trim().replace(/\.+$/, "").toLowerCase()) {
    case "jr":
      return "Jr.";
    case "sr":
      return "Sr.";
    case "ii":
      return "II";
    case "iii":
      return "III";
    case "iv":
      return "IV";
    default:
      return null;
  }
}

export function splitCsv(value: string | null | undefined): string[] {
  if (!value) return [];
  const sep = value.includes(";") ? ";" : ",";
  const seen = new Set<string>();
  const out: string[] = [];
  for (const piece of value.split(sep)) {
    const trimmed = piece.trim();
    if (!trimmed) continue;
    const suffix = sep === "," ? generationalSuffix(trimmed) : null;
    if (suffix && out.length > 0) {
      const prev = out[out.length - 1]!;
      const combined = `${prev} ${suffix}`;
      seen.delete(prev.toLowerCase());
      if (seen.has(combined.toLowerCase())) {
        // Already listed earlier under the combined spelling.
        out.pop();
      } else {
        seen.add(combined.toLowerCase());
        out[out.length - 1] = combined;
      }
      continue;
    }
    const key = trimmed.toLowerCase();
    if (seen.has(key)) continue;
    seen.add(key);
    out.push(trimmed);
  }
  return out;
}

/** First entry of a CSV field, for places with room for one name. */
export function primaryCsvEntry(
  value: string | null | undefined,
): string | null {
  const first = splitCsv(value)[0];
  if (first) return first;
  const trimmed = value?.trim();
  return trimmed ? trimmed : null;
}
