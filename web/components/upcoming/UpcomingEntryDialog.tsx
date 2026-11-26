"use client";

import Link from "next/link";
import { Check, ExternalLink, Loader2 } from "lucide-react";
import * as React from "react";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { UpcomingCover } from "@/components/upcoming/UpcomingCover";
import { UpcomingStageBadge } from "@/components/upcoming/UpcomingStageBadge";
import { useAddSeriesToMylar } from "@/lib/api/mutations";
import type { UpcomingEntryView } from "@/lib/api/types";
import {
  coverMonth,
  entryTitle,
  releaseDay,
  sourceLabel,
  stageSentence,
} from "@/lib/upcoming/stage";
import { seriesUrl } from "@/lib/urls";

/**
 * Everything Folio knows about one upcoming release, with links to the
 * series in Folio and to the release's page on Metron.
 *
 * For an admin with add-to-Mylar set up (`canAddToMylar`), a release whose
 * series Mylar is not tracking also gets "Add series to Mylar": it puts
 * the series on Mylar's watchlist, and Mylar's own settings decide which
 * issues it then fetches. It is the one control on the Upcoming page that
 * acts on Mylar.
 *
 * `entry` stays set while the dialog closes, so the content does not
 * blank during the exit animation.
 */
export function UpcomingEntryDialog({
  entry,
  open,
  onOpenChange,
  canAddToMylar = false,
}: {
  entry: UpcomingEntryView | null;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  canAddToMylar?: boolean;
}) {
  const add = useAddSeriesToMylar();
  // Series queued from this dialog, so the button does not offer them
  // again before the next Mylar snapshot shows them as tracked.
  const [requested, setRequested] = React.useState<ReadonlySet<number>>(
    () => new Set(),
  );
  if (!entry) return null;
  const cvSeriesId = entry.cv_series_id ?? null;
  const offerMylar = canAddToMylar && cvSeriesId != null && !entry.tracked;
  const wasRequested = cvSeriesId != null && requested.has(cvSeriesId);
  const subtitle = [
    entry.publisher,
    entry.year_began ? `${entry.year_began} series` : null,
  ]
    .filter(Boolean)
    .join(" · ");
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>{entryTitle(entry)}</DialogTitle>
          <DialogDescription>
            {subtitle || "Upcoming release"}
          </DialogDescription>
        </DialogHeader>
        <div className="flex gap-4">
          <UpcomingCover entry={entry} className="w-28 shrink-0 self-start" />
          <dl className="min-w-0 flex-1 space-y-3">
            <Fact term="In stores">
              {/* An impossible year, so the year always shows here. */}
              {releaseDay(entry.release_date, "0000")}
            </Fact>
            {entry.cover_date ? (
              <Fact term="Cover date">{coverMonth(entry.cover_date)}</Fact>
            ) : null}
            <Fact term="Status">
              <UpcomingStageBadge entry={entry} />
              <span className="mt-1 block">{stageSentence(entry)}</span>
            </Fact>
            {entry.library_series ? (
              <Fact term="In your library">{entry.library_series.name}</Fact>
            ) : null}
            {entry.tracked && entry.stage !== "tracked" ? (
              <Fact term="Mylar">The series is on the watchlist.</Fact>
            ) : null}
            {canAddToMylar && cvSeriesId == null && !entry.tracked ? (
              <Fact term="Mylar">
                This series has no ComicVine id yet, so it cannot be added to
                Mylar from here.
              </Fact>
            ) : null}
            <Fact term="Listed by">{sourceLabel(entry)}</Fact>
          </dl>
        </div>
        {entry.library_series || entry.metron_url || offerMylar ? (
          <DialogFooter>
            {offerMylar && cvSeriesId != null ? (
              <Button
                variant="outline"
                disabled={add.isPending || wasRequested}
                onClick={() =>
                  add.mutate(
                    {
                      cv_series_id: cvSeriesId,
                      series_name: entry.series_name,
                    },
                    {
                      onSuccess: () =>
                        setRequested((prev) => new Set(prev).add(cvSeriesId)),
                    },
                  )
                }
              >
                {add.isPending ? (
                  <Loader2 className="animate-spin" aria-hidden="true" />
                ) : wasRequested ? (
                  <Check aria-hidden="true" />
                ) : null}
                {wasRequested ? "Requested in Mylar" : "Add series to Mylar"}
              </Button>
            ) : null}
            {entry.metron_url ? (
              <Button variant="outline" asChild>
                <a
                  href={entry.metron_url}
                  target="_blank"
                  rel="noreferrer noopener"
                >
                  View on Metron
                  <ExternalLink aria-hidden="true" />
                </a>
              </Button>
            ) : null}
            {entry.library_series ? (
              <Button asChild>
                <Link href={seriesUrl(entry.library_series.slug)}>
                  Open series
                </Link>
              </Button>
            ) : null}
          </DialogFooter>
        ) : null}
      </DialogContent>
    </Dialog>
  );
}

function Fact({ term, children }: { term: string; children: React.ReactNode }) {
  return (
    <div>
      <dt className="text-muted-foreground text-xs font-medium tracking-wide uppercase">
        {term}
      </dt>
      <dd className="mt-0.5 text-sm">{children}</dd>
    </div>
  );
}
