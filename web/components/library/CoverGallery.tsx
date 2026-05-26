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
 * Image source priority per cover:
 *   1. `source_url` if present (provider CDN — variants typically
 *      have this and not an on-disk artifact yet)
 *   2. else `fallback_primary_url` from the response (page-thumb)
 *
 * Silent when an issue has zero rows AND no fallback is meaningful —
 * the gallery is opt-in surface that simply doesn't appear when
 * there's nothing extra to show.
 */

import { Loader2 } from "lucide-react";
import Image from "next/image";

import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { useIssueCovers } from "@/lib/api/queries";
import type { IssueCoverRow } from "@/lib/api/types";

export function CoverGallery({ issueId }: { issueId: string }) {
  const q = useIssueCovers(issueId);
  const data = q.data;

  // Hide the surface entirely until we either have variants OR we
  // know there's nothing to show.
  if (q.isLoading) {
    return (
      <Card>
        <CardHeader className="pb-2">
          <CardTitle className="text-sm font-medium">Covers</CardTitle>
        </CardHeader>
        <CardContent>
          <div className="text-muted-foreground flex items-center gap-2 py-3 text-sm">
            <Loader2 className="h-4 w-4 animate-spin" /> Loading…
          </div>
        </CardContent>
      </Card>
    );
  }
  if (!data || data.covers.length === 0) {
    return null;
  }
  // If the only cover is the primary slot and we have no variants,
  // skip rendering — the issue page header already shows the primary
  // cover, so a single-tile gallery is redundant.
  const variantCount = data.covers.filter((c) => c.kind !== "primary").length;
  if (variantCount === 0) return null;

  return (
    <Card>
      <CardHeader className="pb-2">
        <CardTitle className="text-sm font-medium">
          Covers ({data.covers.length})
        </CardTitle>
      </CardHeader>
      <CardContent>
        <ul className="grid grid-cols-2 gap-3 sm:grid-cols-3 md:grid-cols-4">
          {data.covers.map((c) => (
            <CoverTile
              key={c.id}
              row={c}
              fallbackUrl={data.fallback_primary_url}
            />
          ))}
        </ul>
      </CardContent>
    </Card>
  );
}

function CoverTile({
  row,
  fallbackUrl,
}: {
  row: IssueCoverRow;
  fallbackUrl: string;
}) {
  const src = row.source_url ?? (row.kind === "primary" ? fallbackUrl : null);
  return (
    <li className="bg-card flex flex-col gap-1.5 rounded-md border p-2">
      <div className="relative aspect-[2/3] overflow-hidden rounded bg-muted">
        {src ? (
          row.source_url ? (
            // External CDN — Next/Image's default loader complains
            // about unknown hosts; use a plain <img> for variants
            // hosted on provider CDNs.
            // eslint-disable-next-line @next/next/no-img-element
            <img
              src={src}
              alt={row.variant_label ?? row.kind}
              loading="lazy"
              className="absolute inset-0 h-full w-full object-cover"
            />
          ) : (
            <Image
              src={src}
              alt={row.variant_label ?? row.kind}
              fill
              className="object-cover"
              sizes="(max-width: 768px) 50vw, 25vw"
              unoptimized
            />
          )
        ) : (
          <div className="h-full w-full" aria-hidden />
        )}
      </div>
      <div className="min-w-0">
        <div className="flex items-center justify-between gap-1 text-xs">
          <span className="truncate font-medium capitalize">
            {row.variant_label ?? row.kind}
          </span>
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
