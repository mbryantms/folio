import { ChevronDown } from "lucide-react";

import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { parseDescription } from "@/lib/description-parser";

/**
 * Renders a series/issue summary, splitting any inline-table content
 * (variant covers lists, etc.) into a collapsible `<details>` so the
 * prose stays readable. Falls through to a plain paragraph when the
 * text has no recognizable structured markers — see `parseDescription`.
 *
 * Server-rendered: uses a native `<details>` element, no client JS.
 */
export function Description({
  text,
  emptyLabel = "No description.",
}: {
  text: string | null | undefined;
  emptyLabel?: string;
}) {
  if (!text || !text.trim()) {
    return <p className="text-muted-foreground text-sm italic">{emptyLabel}</p>;
  }

  const parsed = parseDescription(text);

  if (!parsed.hasStructuredContent) {
    return (
      <p className="text-foreground/90 max-w-prose text-sm leading-6">
        {parsed.intro}
      </p>
    );
  }

  const firstTable = parsed.tables[0];
  const firstSection = parsed.sections[0];
  const totalRows = parsed.tables.reduce((sum, t) => sum + t.rows.length, 0);
  let collapsedLabel = "Show details";
  if (firstTable) {
    collapsedLabel = `Show ${firstTable.title.toLowerCase()}${
      totalRows ? ` (${totalRows})` : ""
    }`;
  } else if (firstSection) {
    collapsedLabel = `Show ${firstSection.title.toLowerCase()}`;
  }

  return (
    <div className="space-y-3">
      {parsed.intro && (
        <p className="text-foreground/90 max-w-prose text-sm leading-6">
          {parsed.intro}
        </p>
      )}
      <details className="group">
        <summary className="border-border text-muted-foreground hover:text-foreground hover:border-foreground/40 inline-flex cursor-pointer list-none items-center gap-1.5 rounded-md border px-2 py-1 text-xs font-medium select-none [&::-webkit-details-marker]:hidden">
          <ChevronDown
            aria-hidden="true"
            className="h-3 w-3 transition-transform group-open:rotate-180"
          />
          <span className="group-open:hidden">{collapsedLabel}</span>
          <span className="hidden group-open:inline">Hide details</span>
        </summary>
        <div className="mt-4 space-y-5">
          {parsed.tables.map((t, i) => (
            <DescriptionTableBlock key={`t-${i}`} table={t} />
          ))}
          {parsed.sections.map((s, i) => (
            <div key={`s-${i}`} className="space-y-1">
              <h4 className="text-muted-foreground text-xs font-semibold tracking-wider uppercase">
                {s.title}
              </h4>
              <p className="text-foreground/90 max-w-prose text-sm leading-6 break-words whitespace-pre-wrap">
                {s.text}
              </p>
            </div>
          ))}
        </div>
      </details>
    </div>
  );
}

function DescriptionTableBlock({ table }: { table: DescriptionTableLike }) {
  return (
    <div className="space-y-2">
      <h4 className="text-muted-foreground text-xs font-semibold tracking-wider uppercase">
        {table.title}
      </h4>
      <div className="border-border overflow-hidden rounded-md border">
        <Table>
          <TableHeader>
            <TableRow>
              {table.columns.map((c, ci) => (
                <TableHead key={ci}>{c}</TableHead>
              ))}
            </TableRow>
          </TableHeader>
          <TableBody>
            {table.rows.map((row, ri) => (
              <TableRow key={ri}>
                {row.map((cell, ci) => (
                  <TableCell key={ci} className="text-sm">
                    {cell || "—"}
                  </TableCell>
                ))}
              </TableRow>
            ))}
          </TableBody>
        </Table>
      </div>
    </div>
  );
}

type DescriptionTableLike = {
  title: string;
  columns: string[];
  rows: string[][];
};
