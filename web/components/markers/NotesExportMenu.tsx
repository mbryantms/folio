"use client";

import { Download } from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { notesExportHref } from "@/lib/urls";

/** "Export" menu on the /bookmarks header (roadmap WP-5.1). Both entries
 *  are plain `<a download>` links to `GET /me/markers/export` — every
 *  marker the user owns, grouped series → issue → page, as Markdown (for a
 *  notes app) or JSON. Shape: `docs/dev/export-format.md` ("Notes
 *  export"). */
export function NotesExportMenu() {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button variant="outline" size="sm" aria-label="Export notes">
          <Download aria-hidden="true" className="mr-1 h-3.5 w-3.5" />
          Export
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        <DropdownMenuItem asChild>
          <a href={notesExportHref("md")} download>
            Markdown (.md)
          </a>
        </DropdownMenuItem>
        <DropdownMenuItem asChild>
          <a href={notesExportHref("json")} download>
            JSON (.json)
          </a>
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
