"use client";

/**
 * `<SourcesFooter>` (metadata-providers-1.0 M5).
 *
 * TOS attribution requirement for ComicVine + Metron. Mandatory on
 * any detail page that uses provider-fetched data — renders as long
 * as the entity has *any* `external_ids` rows from a provider source
 * (CV / Metron / GCD / etc.). Stays absent when the entity has only
 * scanner-derived or user-edited metadata.
 *
 * Renders inline (small, muted) rather than fixed to the viewport
 * since the page footer is the natural place. Per-row links satisfy
 * the "link back to source for every page using the data"
 * requirement.
 */

import * as React from "react";

import type { ExternalIdRow } from "@/lib/api/types";

const ATTRIBUTION_REQUIRED: ReadonlySet<string> = new Set([
  "comicvine",
  "metron",
  "gcd",
  "marvel",
  "locg",
]);

export function SourcesFooter({ rows }: { rows: ExternalIdRow[] | undefined }) {
  const attributable = (rows ?? []).filter((r) =>
    ATTRIBUTION_REQUIRED.has(r.source),
  );
  if (attributable.length === 0) return null;
  return (
    <div className="text-muted-foreground mt-6 border-t pt-3 text-xs">
      Data from{" "}
      {attributable.map((r, i) => (
        <React.Fragment key={r.source}>
          {r.external_url ? (
            <a
              href={r.external_url}
              target="_blank"
              rel="noreferrer"
              className="underline-offset-2 hover:underline"
            >
              {r.source_label}
            </a>
          ) : (
            <span>{r.source_label}</span>
          )}
          {i < attributable.length - 1 ? ", " : ""}
        </React.Fragment>
      ))}
      .
    </div>
  );
}
