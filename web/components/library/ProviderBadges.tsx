"use client";

/**
 * `<ProviderBadges>` (metadata-providers-1.0 M5.2).
 *
 * Tiny linked-source pill row. Renders the per-row `external_url`
 * (CV / Metron / GCD / Marvel / LoCG) so the user can jump to the
 * authoritative source. Used on detail pages (next to the title) +
 * the per-entity sidebar — anywhere there's room for a 4-character
 * abbreviation pill.
 *
 * Silent when the entity has zero provider rows; lets the component
 * be mounted unconditionally without empty boxes.
 */

import type { ExternalIdRow } from "@/lib/api/types";

const ATTRIBUTION_REQUIRED: ReadonlySet<string> = new Set([
  "comicvine",
  "metron",
  "gcd",
  "marvel",
  "locg",
]);

const SHORT_LABELS: Record<string, string> = {
  comicvine: "CV",
  metron: "Metron",
  gcd: "GCD",
  marvel: "Marvel",
  locg: "LoCG",
};

export function ProviderBadges({
  rows,
  className,
}: {
  rows: ExternalIdRow[] | undefined;
  className?: string;
}) {
  const visible = (rows ?? []).filter((r) =>
    ATTRIBUTION_REQUIRED.has(r.source),
  );
  if (visible.length === 0) return null;
  return (
    <ul className={`flex flex-wrap items-center gap-1 ${className ?? ""}`}>
      {visible.map((r) => (
        <li key={r.source}>
          {r.external_url ? (
            <a
              href={r.external_url}
              target="_blank"
              rel="noreferrer"
              className="border-border text-muted-foreground hover:bg-muted hover:text-foreground rounded border px-1.5 py-0.5 text-[10px] font-medium uppercase leading-none transition-colors"
              title={`${r.source_label} · ${r.external_id}`}
            >
              {SHORT_LABELS[r.source] ?? r.source.toUpperCase()}
            </a>
          ) : (
            <span
              className="border-border text-muted-foreground rounded border px-1.5 py-0.5 text-[10px] font-medium uppercase leading-none"
              title={`${r.source_label} · ${r.external_id}`}
            >
              {SHORT_LABELS[r.source] ?? r.source.toUpperCase()}
            </span>
          )}
        </li>
      ))}
    </ul>
  );
}
