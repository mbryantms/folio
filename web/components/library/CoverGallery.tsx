"use client";

/**
 * `<CoverGallery>` (metadata-providers-1.0 M5.2).
 *
 * Renders every `issue_cover` row for an issue as a thumbnail grid.
 * Primary first, variants after, sorted by `kind, ordinal`. Each
 * tile shows the variant label + artist credit when present.
 *
 * Current state: the gallery is mostly populated by M4 Apply jobs,
 * which today only persist the **primary** cover (variants aren't
 * yet written back to `issue_cover`). The grid still renders the
 * primary row + any backfilled rows + acts as the surface variants
 * will appear in once M4.x adds variant persistence — no UI changes
 * needed when that lands.
 *
 * Both the thumbnail and the "open full resolution" link use the same
 * source per cover:
 *   1. `image_url` from the response — the local byte endpoint
 *      (`/issues/{id}/covers/{cover_id}`) once the cover is downloaded
 *      (the full-res original), else the provider CDN hotlink
 *      (`source_url`) for not-yet-localized rows
 *   2. else `fallback_primary_url` from the response (page-thumb)
 *
 * Silent when an issue has zero rows AND no fallback is meaningful —
 * the gallery is opt-in surface that simply doesn't appear when
 * there's nothing extra to show.
 */

import { Loader2 } from "lucide-react";
import { useState } from "react";

import {
  CoverViewer,
  type ViewerCover,
} from "@/components/library/CoverViewer";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { useIssueCovers } from "@/lib/api/queries";
import type { IssueCoverRow } from "@/lib/api/types";

/** The full-resolution source for a cover row, or `null` when none exists. */
function coverSrc(row: IssueCoverRow, fallbackUrl: string): string | null {
  return row.image_url ?? (row.kind === "primary" ? fallbackUrl : null);
}

/**
 * `chrome` controls the outer wrapper:
 *   - `"card"` (default): `<Card>` with a "Covers ({n})" header. Used by the
 *     legacy panel layout + standalone surfaces. Returns `null` when the
 *     issue has only a primary cover (no variants to show — the page
 *     header already displays the primary).
 *   - `"bare"`: drops the `<Card>` chrome. Caller owns the section title
 *     (e.g. the parent tab label is "Covers"). Always renders even when
 *     only the primary exists, because tab content shouldn't silently
 *     disappear after the user clicked the tab.
 */
export function CoverGallery({
  issueId,
  chrome = "card",
}: {
  issueId: string;
  chrome?: "card" | "bare";
}) {
  const q = useIssueCovers(issueId);
  const data = q.data;
  // Active lightbox cover (index into the `viewable` list below), or null when
  // closed. Declared before the early returns so hook order stays stable.
  const [viewerIndex, setViewerIndex] = useState<number | null>(null);

  if (q.isLoading) {
    const loading = (
      <div className="text-muted-foreground flex items-center gap-2 py-3 text-sm">
        <Loader2 className="h-4 w-4 animate-spin" /> Loading…
      </div>
    );
    if (chrome === "bare") return loading;
    return (
      <Card>
        <CardHeader className="pb-2">
          <CardTitle className="text-sm font-medium">Covers</CardTitle>
        </CardHeader>
        <CardContent>{loading}</CardContent>
      </Card>
    );
  }

  if (!data || data.covers.length === 0) {
    if (chrome === "bare") {
      // Tab content: render an empty-state instead of silently
      // collapsing — clicking the tab and seeing nothing is worse than
      // an explicit "No covers yet" message.
      return (
        <p className="text-muted-foreground text-sm">
          No covers recorded for this issue yet. Run a metadata fetch from a
          provider that returns variant covers (Metron) to populate this list.
        </p>
      );
    }
    return null;
  }

  // Card mode: hide the gallery when only the primary exists (the
  // issue-page header already shows it; a single-tile gallery is
  // redundant). Bare mode always renders because the tab label is
  // already a commitment to show something.
  const variantCount = data.covers.filter((c) => c.kind !== "primary").length;
  if (chrome === "card" && variantCount === 0) return null;

  // Denser grid in bare mode (the issue-page tab gives us full
  // page-width, so we can fit 4-7 columns at common breakpoints).
  // Card mode keeps the original sizing for the series-page panel +
  // any legacy 2-col-grid caller.
  const gridClass =
    chrome === "bare"
      ? "grid grid-cols-3 gap-3 sm:grid-cols-4 md:grid-cols-5 lg:grid-cols-6 xl:grid-cols-7"
      : "grid grid-cols-2 gap-3 sm:grid-cols-3 md:grid-cols-4";

  // Build the ordered list of viewable covers (those with an image) once, plus
  // a map from cover id → its position in that list, so a tile click can open
  // the lightbox at the right slide. Covers without an image aren't clickable.
  const viewable: ViewerCover[] = [];
  const viewerIndexById = new Map<string, number>();
  for (const c of data.covers) {
    const src = coverSrc(c, data.fallback_primary_url);
    if (!src) continue;
    viewerIndexById.set(c.id, viewable.length);
    viewable.push({
      src,
      label: c.variant_label ?? c.kind,
      provider: c.source_provider ? labelForProvider(c.source_provider) : null,
    });
  }

  const grid = (
    <ul className={gridClass}>
      {data.covers.map((c) => {
        const vi = viewerIndexById.get(c.id);
        return (
          <CoverTile
            key={c.id}
            row={c}
            fallbackUrl={data.fallback_primary_url}
            onOpen={vi === undefined ? null : () => setViewerIndex(vi)}
          />
        );
      })}
    </ul>
  );

  const viewer = (
    <CoverViewer
      covers={viewable}
      index={viewerIndex}
      onIndexChange={setViewerIndex}
      onClose={() => setViewerIndex(null)}
    />
  );

  if (chrome === "bare")
    return (
      <>
        {grid}
        {viewer}
      </>
    );

  return (
    <>
      <Card>
        <CardHeader className="pb-2">
          <CardTitle className="text-sm font-medium">
            Covers ({data.covers.length})
          </CardTitle>
        </CardHeader>
        <CardContent>{grid}</CardContent>
      </Card>
      {viewer}
    </>
  );
}

function CoverTile({
  row,
  fallbackUrl,
  onOpen,
}: {
  row: IssueCoverRow;
  fallbackUrl: string;
  /** Opens the lightbox at this cover; `null` when the cover has no image. */
  onOpen: (() => void) | null;
}) {
  // The thumbnail and the lightbox use the same `image_url` — the locally
  // stored cover, which is the full-res original we downloaded (the CDN
  // `source_url` is only the fallback baked into `image_url` for
  // not-yet-localized rows). Primary covers with no stored artifact fall back
  // to the page-thumb route.
  const src = coverSrc(row, fallbackUrl);
  const label = row.variant_label ?? row.kind;

  // Image element — drops the wrapping border / padding / bg-card the
  // legacy tile used; the image itself is the tile. `src` may be a local
  // same-origin cover route or an external CDN hotlink (soft-fallback
  // rows); a plain <img> serves both without Next/Image host config.
  const img = src ? (
    // eslint-disable-next-line @next/next/no-img-element
    <img
      src={src}
      alt={label}
      loading="lazy"
      className="absolute inset-0 h-full w-full object-cover"
    />
  ) : (
    <div className="h-full w-full" aria-hidden />
  );

  const caption = (
    <div className="mt-1.5 min-w-0">
      <div className="flex items-center justify-between gap-1 text-xs">
        <span className="truncate capitalize">{label}</span>
        {row.kind !== "primary" && row.ordinal > 0 && (
          <span className="text-muted-foreground">#{row.ordinal}</span>
        )}
      </div>
      {row.source_provider && (
        <p className="text-muted-foreground truncate text-xs">
          {labelForProvider(row.source_provider)}
        </p>
      )}
    </div>
  );

  const frame = (
    <div className="bg-muted relative aspect-2/3 overflow-hidden rounded">
      {img}
    </div>
  );

  return (
    <li>
      {onOpen ? (
        // Open the full-resolution image in the in-app lightbox. A new-tab
        // link would strand PWA users on the chromeless image-bytes endpoint
        // with no way back; the viewer keeps the full-res view inside the app.
        <button
          type="button"
          onClick={onOpen}
          className="group focus-visible:outline-ring block w-full rounded text-left focus-visible:outline-2 focus-visible:outline-offset-2"
          title={`View ${label}`}
        >
          <div className="group-hover:ring-ring/40 rounded transition-shadow group-hover:ring-2">
            {frame}
          </div>
          {caption}
        </button>
      ) : (
        <>
          {frame}
          {caption}
        </>
      )}
    </li>
  );
}

function labelForProvider(p: string): string {
  switch (p) {
    case "comicvine":
      return "ComicVine";
    case "metron":
      return "Metron";
    case "archive_extracted":
      return "Archive";
    case "user_upload":
      return "User upload";
    default:
      return p;
  }
}
