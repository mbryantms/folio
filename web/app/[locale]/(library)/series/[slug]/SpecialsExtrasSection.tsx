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

/** One section per `special_type`, in this fixed order; unknown types
 *  follow alphabetically. The label is the section heading. */
const SPECIAL_SECTIONS: Array<{ type: string; label: string; blurb: string }> =
  [
    {
      type: "Annual",
      label: "Annuals",
      blurb: "Yearly extras, numbered on their own.",
    },
    {
      type: "Special",
      label: "Specials",
      blurb: "Tie-ins, bonus material and specials.",
    },
    { type: "OneShot", label: "One-shots", blurb: "Standalone issues." },
    {
      type: "TPB",
      label: "Collected editions",
      blurb: "Trades and graphic novels.",
    },
  ];

export type SpecialGroup = {
  type: string;
  label: string;
  blurb: string | null;
  items: IssueSummaryView[];
};

/** Issue-number ascending inside a group (annuals read #1, #2, …), with a
 *  stable title / id tiebreaker. */
function byNumber(a: IssueSummaryView, b: IssueSummaryView): number {
  const na = a.sort_number ?? Number.POSITIVE_INFINITY;
  const nb = b.sort_number ?? Number.POSITIVE_INFINITY;
  if (na !== nb) return na - nb;
  const ka = a.title ?? a.number ?? a.id;
  const kb = b.title ?? b.number ?? b.id;
  return ka.localeCompare(kb);
}

/**
 * Group tagged specials into ordered sections: Annuals, Specials,
 * One-shots, Collected editions, then any other type alphabetically.
 * Empty groups are dropped.
 */
export function groupSpecials(items: IssueSummaryView[]): SpecialGroup[] {
  const byType = new Map<string, IssueSummaryView[]>();
  for (const item of items) {
    const type = item.special_type ?? "";
    if (!type) continue;
    const list = byType.get(type) ?? [];
    list.push(item);
    byType.set(type, list);
  }
  const groups: SpecialGroup[] = [];
  for (const s of SPECIAL_SECTIONS) {
    const list = byType.get(s.type);
    if (list?.length) {
      groups.push({
        type: s.type,
        label: s.label,
        blurb: s.blurb,
        items: [...list].sort(byNumber),
      });
      byType.delete(s.type);
    }
  }
  for (const type of Array.from(byType.keys()).sort((a, b) =>
    a.localeCompare(b),
  )) {
    const list = byType.get(type)!;
    groups.push({
      type,
      label: type,
      blurb: null,
      items: [...list].sort(byNumber),
    });
  }
  return groups;
}

/**
 * "Specials & Extras" — annuals, one-shots, specials and collected
 * editions the scanner classified via ComicInfo `<Format>` or a
 * recognized subfolder (`Annuals/`, `Specials/`, `Oneshots/`). They are
 * not part of the series' run, so they get their own sections below the
 * main grid, one per type with a count. Hidden when empty. `renderCard`
 * lets the issues panel reuse its select-mode card so bulk actions cover
 * specials too.
 */
export function SpecialsExtrasSection({
  items,
  gridStyle,
  renderCard,
}: {
  items: IssueSummaryView[];
  gridStyle: React.CSSProperties;
  renderCard?: (issue: IssueSummaryView) => React.ReactNode;
}) {
  const groups = groupSpecials(items);
  if (groups.length === 0) return null;
  const render =
    renderCard ?? ((iss: IssueSummaryView) => <IssueCard issue={iss} />);
  return (
    <section
      aria-labelledby="specials-extras-heading"
      data-testid="specials-extras-section"
      className="mt-10 space-y-8"
    >
      <div>
        <h3
          id="specials-extras-heading"
          className="text-base font-semibold tracking-tight"
        >
          Specials &amp; Extras
        </h3>
        <p className="text-muted-foreground text-xs">
          Not part of the numbered run. Discovered from ComicInfo{" "}
          <span className="font-mono">&lt;Format&gt;</span> or a recognized
          subfolder (<span className="font-mono">Annuals/</span>,{" "}
          <span className="font-mono">Specials/</span>,{" "}
          <span className="font-mono">Oneshots/</span>).
        </p>
      </div>
      {groups.map((g) => (
        <div key={g.type} data-testid={`specials-group-${g.type}`}>
          <h4 className="text-sm font-semibold tracking-tight">
            {g.label}{" "}
            <span className="text-muted-foreground font-normal tabular-nums">
              ({g.items.length})
            </span>
          </h4>
          {g.blurb && (
            <p className="text-muted-foreground mb-3 text-xs">{g.blurb}</p>
          )}
          <ul role="list" className="mt-2 grid gap-4" style={gridStyle}>
            {g.items.map((iss) => (
              <li key={iss.id}>{render(iss)}</li>
            ))}
          </ul>
        </div>
      ))}
    </section>
  );
}
