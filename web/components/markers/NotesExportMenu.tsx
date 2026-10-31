"use client";

import { Download, MoreHorizontal } from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/folio/dropdown-menu";
import { notesExportHref } from "@/lib/urls";

/** "Export" menu on the /bookmarks header (roadmap WP-5.1). Both entries
 *  are plain `<a download>` links to `GET /me/markers/export` — every
 *  marker the user owns, grouped series → issue → page, as Markdown (for a
 *  notes app) or JSON. Shape: `docs/dev/export-format.md` ("Notes
 *  export"). */
export function NotesExportMenu({
  variant = "more",
}: {
  /** `more` (default): the page's `⋯` More-actions trigger — Export is
   *  an action on the page's subject, so it lives behind `⋯` like every
   *  other view's export (rule 2). `button`: the labelled "Export"
   *  button, for surfaces that have no other `⋯`. */
  variant?: "more" | "button";
}) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        {variant === "more" ? (
          <Button
            variant="outline"
            size="icon"
            className="h-9 w-9 shrink-0"
            aria-label="More actions"
            title="More actions"
          >
            <MoreHorizontal aria-hidden="true" className="size-4" />
          </Button>
        ) : (
          <Button variant="outline" aria-label="Export notes">
            <Download aria-hidden="true" className="mr-1 size-3.5" />
            Export
          </Button>
        )}
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        <DropdownMenuItem asChild>
          <a href={notesExportHref("md")} download>
            <Download aria-hidden="true" className="size-4" />
            Export notes as Markdown (.md)
          </a>
        </DropdownMenuItem>
        <DropdownMenuItem asChild>
          <a href={notesExportHref("json")} download>
            <Download aria-hidden="true" className="size-4" />
            Export notes as JSON (.json)
          </a>
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
