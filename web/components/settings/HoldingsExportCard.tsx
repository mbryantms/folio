"use client";

import { useState } from "react";
import { Download } from "lucide-react";

import { NativeSelect } from "@/components/folio/native-select";
import { Button } from "@/components/ui/button";
import { useLibraryList } from "@/lib/api/queries";

import { SettingsSection } from "./SettingsSection";

export type HoldingsScope = "series" | "issues";

/** Same-origin path of the holdings CSV. Like the account export it is a
 *  plain `<a download>`: the session cookie rides along, the browser
 *  streams the body straight to disk (no fetch-to-blob, so a 50k-issue
 *  library never sits in page memory) and saves it under the server's
 *  `folio-holdings-<scope>-<date>.csv` filename. */
export function holdingsHref(scope: HoldingsScope, library: string): string {
  const params = new URLSearchParams({ scope });
  if (library) params.set("library", library);
  return `/api/me/holdings.csv?${params.toString()}`;
}

const SCOPE_OPTIONS = [
  { value: "series", label: "Series" },
  { value: "issues", label: "Issues" },
];

/** Holdings export (product audit 2026-11 §5 item 14): what this account can
 *  see in the library, as a spreadsheet. One row per series or per issue,
 *  optionally limited to one library. */
export function HoldingsExportCard() {
  const [scope, setScope] = useState<HoldingsScope>("series");
  const [library, setLibrary] = useState("");
  const libraries = useLibraryList();
  const libraryOptions = [
    { value: "", label: "All libraries" },
    ...(libraries.data ?? []).map((l) => ({ value: l.slug, label: l.name })),
  ];

  return (
    <SettingsSection
      title="Holdings"
      description="Download a CSV of the series or issues you can see, for a spreadsheet, an insurance record, or checking against a want list."
    >
      <div className="flex flex-col gap-4">
        <p className="text-muted-foreground text-sm">
          Series rows carry publisher, volume, start year, status, issues owned
          and read, and ComicVine, Metron and GCD ids. Issue rows carry number,
          title, cover date, format, pages, file size, read state and the same
          provider ids. The file is UTF-8 and opens directly in Excel, Numbers
          or LibreOffice.
        </p>
        <div className="flex flex-col gap-3 sm:flex-row sm:items-end">
          <label className="flex flex-col gap-1.5 text-sm">
            <span className="text-muted-foreground">One row per</span>
            <NativeSelect
              aria-label="One row per"
              value={scope}
              onChange={(v) => setScope(v === "issues" ? "issues" : "series")}
              options={SCOPE_OPTIONS}
            />
          </label>
          <label className="flex flex-col gap-1.5 text-sm">
            <span className="text-muted-foreground">Library</span>
            <NativeSelect
              aria-label="Library"
              value={library}
              onChange={setLibrary}
              options={libraryOptions}
            />
          </label>
          <Button variant="outline" asChild className="shrink-0">
            <a href={holdingsHref(scope, library)} download>
              <Download aria-hidden="true" className="mr-1 size-3.5" />
              Download holdings
            </a>
          </Button>
        </div>
      </div>
    </SettingsSection>
  );
}
