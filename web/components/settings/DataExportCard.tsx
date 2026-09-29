import { Download } from "lucide-react";

import { Button } from "@/components/ui/button";

import { SettingsSection } from "./SettingsSection";

/** Same-origin path of the account export. A plain `<a download>` carries
 *  the session cookie, so no client fetch/auth plumbing is needed — the
 *  browser saves the response under the server's `Content-Disposition`
 *  filename (`folio-export-<date>.json`). */
export const EXPORT_HREF = "/api/me/export";

/** Data-liberation card (roadmap WP-2.1). One click downloads a single JSON
 *  document with everything this account owns; the shape is documented in
 *  `docs/dev/export-format.md`. */
export function DataExportCard() {
  return (
    <SettingsSection
      title="Your data"
      description="Download everything Folio stores for this account as one JSON file."
    >
      <div className="flex flex-col gap-4 sm:flex-row sm:items-center sm:justify-between">
        <p className="text-muted-foreground text-sm">
          Reading progress, notes and bookmarks, collections and Want to Read,
          saved views, ratings, custom pages and sidebar layout, reading log,
          and preferences including keybinds. Every issue is keyed by content
          hash and series name, year, and number so the file stays useful after
          a library rebuild.
        </p>
        <Button variant="outline" size="sm" asChild className="shrink-0">
          <a href={EXPORT_HREF} download>
            <Download aria-hidden="true" className="mr-1 h-3.5 w-3.5" />
            Export my data
          </a>
        </Button>
      </div>
    </SettingsSection>
  );
}
