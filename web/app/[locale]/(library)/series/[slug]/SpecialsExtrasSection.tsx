"use client";

import { IssueCard } from "@/components/library/IssueCard";
import type { IssueSummaryView } from "@/lib/api/types";

/**
 * Split issues into main-run and specials/extras buckets. Mirrors the
 * server-side `special_type` classification (spec §6.5). An issue is
 * "main-run" when `special_type` is null/undefined/empty; everything
 * else is an extra.
 */
export function splitMainAndSpecials(items: IssueSummaryView[]): {
  mainRun: IssueSummaryView[];
  specials: IssueSummaryView[];
} {
  const mainRun: IssueSummaryView[] = [];
  const specials: IssueSummaryView[] = [];
  for (const item of items) {
    if (item.special_type) {
      specials.push(item);
    } else {
      mainRun.push(item);
    }
  }
  return { mainRun, specials: sortSpecials(specials) };
}

/**
 * Stable order for the Specials & Extras section: `special_type`
 * ascending (so Annuals group above OneShots etc.), then a per-item
 * tiebreaker so two annuals don't shuffle between renders.
 */
export function sortSpecials(items: IssueSummaryView[]): IssueSummaryView[] {
  return [...items].sort((a, b) => {
    const ta = a.special_type ?? "";
    const tb = b.special_type ?? "";
    if (ta !== tb) return ta.localeCompare(tb);
    const ka = a.title ?? a.number ?? a.id;
    const kb = b.title ?? b.number ?? b.id;
    return ka.localeCompare(kb);
  });
}

/**
 * "Specials & Extras" — annuals, one-shots, art books, and tie-ins
 * that the scanner classified via ComicInfo `<Format>` or the
 * path-derived rule (M2.5 of scanner-nested-folders). Hidden when
 * empty; read-only in v1 (no selection, no filter chips, no edits).
 *
 * Sorting comes from [`sortSpecials`] so two annuals don't shuffle
 * between renders.
 */
export function SpecialsExtrasSection({
  items,
  gridStyle,
}: {
  items: IssueSummaryView[];
  gridStyle: React.CSSProperties;
}) {
  if (items.length === 0) return null;
  return (
    <section
      aria-labelledby="specials-extras-heading"
      data-testid="specials-extras-section"
      className="mt-10"
    >
      <h3
        id="specials-extras-heading"
        className="text-base font-semibold tracking-tight"
      >
        Specials &amp; Extras
      </h3>
      <p className="text-muted-foreground mb-3 text-xs">
        Annuals, one-shots, tie-ins, and bonus material. Discovered from
        ComicInfo <span className="font-mono">&lt;Format&gt;</span> or
        from a recognized subfolder (
        <span className="font-mono">Specials/</span>,{" "}
        <span className="font-mono">Annuals/</span>,{" "}
        <span className="font-mono">Oneshots/</span>).
      </p>
      <ul role="list" className="grid gap-4" style={gridStyle}>
        {items.map((iss) => (
          <li key={iss.id}>
            <IssueCard issue={iss} />
          </li>
        ))}
      </ul>
    </section>
  );
}
