"use client";

import Link from "next/link";
import { ExternalLink } from "lucide-react";

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
 * Everything Folio knows about one upcoming release. Read-only: the two
 * links lead to the series in Folio and to the release's page on Metron.
 * Nothing here reaches Mylar.
 *
 * `entry` stays set while the dialog closes, so the content does not
 * blank during the exit animation.
 */
export function UpcomingEntryDialog({
  entry,
  open,
  onOpenChange,
}: {
  entry: UpcomingEntryView | null;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  if (!entry) return null;
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
            <Fact term="Listed by">{sourceLabel(entry)}</Fact>
          </dl>
        </div>
        {entry.library_series || entry.metron_url ? (
          <DialogFooter>
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
