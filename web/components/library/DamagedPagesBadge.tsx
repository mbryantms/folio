"use client";

import { AlertTriangle } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/folio/popover";
import type { DamagedPageView } from "@/lib/api/types";
import { damagedPageLine } from "@/lib/library/damaged-pages";
import { statusTone } from "@/lib/ui/status-tone";

/**
 * Issue-page chip for pages Folio kept from a damaged archive
 * (`issue.damaged_pages`). The file's original pages failed their
 * checksum; Folio converted it anyway, keeping what could be recovered or
 * a stand-in page. The chip opens a popover (works on touch, unlike a
 * tooltip) naming each page and what to do about it. Renders nothing for
 * an undamaged issue.
 */
export function DamagedPagesBadge({ pages }: { pages: DamagedPageView[] }) {
  if (pages.length === 0) return null;
  const label =
    pages.length === 1 ? "1 damaged page" : `${pages.length} damaged pages`;
  return (
    <Popover>
      <PopoverTrigger asChild>
        <Badge asChild variant="outline" className={statusTone("warning")}>
          <button type="button" data-testid="damaged-pages-badge">
            <AlertTriangle aria-hidden="true" className="mr-1 size-3" />
            {label}
          </button>
        </Badge>
      </PopoverTrigger>
      <PopoverContent align="start" className="w-80 text-sm">
        <div className="space-y-2">
          <p className="font-medium">
            {pages.length === 1
              ? "This file has a damaged page"
              : "This file has damaged pages"}
          </p>
          <ul className="text-muted-foreground space-y-1">
            {pages.map((p) => (
              <li key={p.page_index}>{damagedPageLine(p)}</li>
            ))}
          </ul>
          <p className="text-muted-foreground text-xs">
            The original file failed its checksum on{" "}
            {pages.length === 1 ? "this page" : "these pages"}, so Folio kept
            what it could. Replace the file with a good copy and rescan to clear
            this.
          </p>
        </div>
      </PopoverContent>
    </Popover>
  );
}
