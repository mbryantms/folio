"use client";

import {
  Item,
  ItemActions,
  ItemContent,
  ItemDescription,
  ItemMedia,
  ItemTitle,
} from "@/components/ui/item";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { UpcomingCover } from "@/components/upcoming/UpcomingCover";
import { UpcomingStageBadge } from "@/components/upcoming/UpcomingStageBadge";
import type { UpcomingEntryView } from "@/lib/api/types";
import { entryTitle, releaseDay } from "@/lib/upcoming/stage";

/**
 * One week of releases as rows: a table from `sm` up, stacked rows below
 * (two compositions, docs/dev/phone-layout.md rule 1). The release title
 * is the control that opens the detail dialog in both.
 */
export function UpcomingList({
  entries,
  today,
  caption,
  onOpen,
}: {
  entries: UpcomingEntryView[];
  today: string;
  /** Names the table for assistive tech, e.g. "This week". */
  caption: string;
  onOpen: (entry: UpcomingEntryView) => void;
}) {
  return (
    <>
      <div className="max-sm:hidden">
        <Table aria-label={caption}>
          <TableHeader>
            <TableRow>
              <TableHead className="w-28">In stores</TableHead>
              {/* The title takes the row's spare width (and truncates
                  inside it: `max-w-0` on its cells). */}
              <TableHead className="w-full">Issue</TableHead>
              <TableHead className="min-w-40 max-lg:hidden">
                Publisher
              </TableHead>
              <TableHead className="w-32">Status</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {entries.map((entry) => (
              <TableRow key={entry.key} data-upcoming-key={entry.key}>
                <TableCell className="text-muted-foreground whitespace-nowrap tabular-nums">
                  {releaseDay(entry.release_date, today)}
                </TableCell>
                <TableCell className="w-full max-w-0">
                  <button
                    type="button"
                    aria-haspopup="dialog"
                    onClick={() => onOpen(entry)}
                    className="focus-visible:ring-ring block max-w-full truncate rounded-sm text-left font-medium underline-offset-4 hover:underline focus-visible:ring-2 focus-visible:outline-none"
                  >
                    {entryTitle(entry)}
                  </button>
                </TableCell>
                <TableCell className="text-muted-foreground whitespace-nowrap max-lg:hidden">
                  {entry.publisher ?? ""}
                </TableCell>
                <TableCell>
                  <UpcomingStageBadge entry={entry} />
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      </div>

      <ul className="space-y-2 sm:hidden">
        {entries.map((entry) => (
          <li key={entry.key}>
            <Item variant="outline" size="sm" asChild>
              <button
                type="button"
                aria-haspopup="dialog"
                data-upcoming-key={entry.key}
                onClick={() => onOpen(entry)}
                className="hover:bg-accent/40 text-left"
              >
                <ItemMedia>
                  <UpcomingCover
                    entry={entry}
                    className="w-10 rounded-sm text-[10px]"
                  />
                </ItemMedia>
                <ItemContent className="min-w-0">
                  <ItemTitle className="w-full">
                    <span className="truncate">{entryTitle(entry)}</span>
                  </ItemTitle>
                  <ItemDescription className="truncate">
                    {[releaseDay(entry.release_date, today), entry.publisher]
                      .filter(Boolean)
                      .join(" · ")}
                  </ItemDescription>
                </ItemContent>
                <ItemActions>
                  <UpcomingStageBadge entry={entry} />
                </ItemActions>
              </button>
            </Item>
          </li>
        ))}
      </ul>
    </>
  );
}
