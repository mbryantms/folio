"use client";

/**
 * `<SeriesSuggestedRelationships>` — WP-7.3 admin-only "Suggested" chips in
 * the series page's Related block: the pending relationship suggestions
 * with this series on either end (`GET /series/{slug}/relationship-
 * suggestions`, cursor-paginated — "Show more" walks the next page).
 *
 * Each chip reads from this series' point of view ("Sequel to Daredevil")
 * — a suggestion stored as `other kind this` shows the inverse kind — with
 * the engine's reason in a tooltip, and one-click accept / reject. Accept
 * invalidates both series' relationship blocks and the similar-series
 * rails (see `useAcceptRelationshipSuggestion`).
 */

import { Check, Loader2, Sparkles, X } from "lucide-react";
import Link from "next/link";
import * as React from "react";

import { Button } from "@/components/ui/button";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import {
  useAcceptRelationshipSuggestion,
  useRejectRelationshipSuggestion,
} from "@/lib/api/mutations";
import { useSeriesRelationshipSuggestions } from "@/lib/api/queries";
import type {
  RelationshipKind,
  RelationshipSuggestionView,
  SeriesView,
} from "@/lib/api/types";
import { seriesUrl } from "@/lib/urls";

/** The suggestion as seen from `seriesId`: the kind this series would
 *  have, and the other series. */
export function fromPerspective(
  s: RelationshipSuggestionView,
  seriesId: string,
): { kind: RelationshipKind; label: string; other: SeriesView } {
  const isFrom = s.from_series.id === seriesId;
  return {
    kind: isFrom ? s.kind : s.inverse_kind,
    label: isFrom ? s.kind_label : s.inverse_kind_label,
    other: isFrom ? s.to_series : s.from_series,
  };
}

export function SeriesSuggestedRelationships({
  seriesSlug,
  seriesId,
}: {
  seriesSlug: string;
  seriesId: string;
}) {
  const query = useSeriesRelationshipSuggestions(seriesSlug, true);
  const items = query.data?.pages.flatMap((p) => p.items) ?? [];
  if (query.isLoading || query.error || items.length === 0) return null;

  return (
    <div className="space-y-2" data-testid="suggested-relationships">
      <h3 className="text-muted-foreground flex items-center gap-1 text-xs font-medium tracking-wider uppercase">
        <Sparkles aria-hidden className="h-3 w-3" /> Suggested
      </h3>
      <TooltipProvider delayDuration={250}>
        <ul className="flex flex-wrap gap-2">
          {items.map((s) => (
            <SuggestionChip key={s.id} suggestion={s} seriesId={seriesId} />
          ))}
        </ul>
      </TooltipProvider>
      {query.hasNextPage ? (
        <Button
          variant="ghost"
          size="sm"
          disabled={query.isFetchingNextPage}
          onClick={() => void query.fetchNextPage()}
        >
          {query.isFetchingNextPage ? "Loading…" : "Show more"}
        </Button>
      ) : null}
    </div>
  );
}

function SuggestionChip({
  suggestion: s,
  seriesId,
}: {
  suggestion: RelationshipSuggestionView;
  seriesId: string;
}) {
  const accept = useAcceptRelationshipSuggestion();
  const reject = useRejectRelationshipSuggestion();
  const busy = accept.isPending || reject.isPending;
  const { label, other } = fromPerspective(s, seriesId);
  const otherName = `${other.name}${other.year != null ? ` (${other.year})` : ""}`;

  return (
    <li className="border-border bg-card flex items-center gap-1 rounded-full border border-dashed py-0.5 pr-1 pl-3 text-sm">
      <Tooltip>
        <TooltipTrigger asChild>
          <Link href={seriesUrl(other)} className="hover:underline">
            <span className="text-muted-foreground">{label}</span>{" "}
            <span className="font-medium">{otherName}</span>
          </Link>
        </TooltipTrigger>
        <TooltipContent className="max-w-xs">
          <p>{s.reason}</p>
          <p className="text-muted-foreground mt-1">
            {Math.round(s.confidence * 100)}% · {s.bucket} confidence
          </p>
        </TooltipContent>
      </Tooltip>
      <Button
        variant="ghost"
        size="icon"
        className="h-6 w-6"
        disabled={busy}
        aria-label={`Accept: ${label} ${otherName}`}
        onClick={() => accept.mutate({ id: s.id })}
      >
        {accept.isPending ? (
          <Loader2 className="h-3.5 w-3.5 animate-spin" />
        ) : (
          <Check className="h-3.5 w-3.5" />
        )}
      </Button>
      <Button
        variant="ghost"
        size="icon"
        className="text-muted-foreground h-6 w-6"
        disabled={busy}
        aria-label={`Reject: ${label} ${otherName}`}
        onClick={() => reject.mutate({ id: s.id })}
      >
        <X className="h-3.5 w-3.5" />
      </Button>
    </li>
  );
}
