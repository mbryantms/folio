"use client";

/**
 * "Appears in" tab — shows the reading lists, collections, and story arcs a
 * given issue or series belongs to, so a reader browsing organically can
 * discover and jump to the other lists it's part of.
 *
 * Reading lists and collections are the user's own (scoped server-side) and
 * open at `/views/{id}`. Story arcs are shared metadata with no detail route
 * yet, so they render as informational chips rather than links.
 *
 * One component drives both detail pages via `variant`: the issue side shows
 * the issue's reading-order position within a list/arc; the series side shows
 * how many of the series' issues each container holds.
 */

import Link from "next/link";
import { BookMarked, Layers, Library, Loader2 } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { useIssueAppearances, useSeriesAppearances } from "@/lib/api/queries";
import type { AppearanceView } from "@/lib/api/types";

type Props =
  | { variant: "issue"; seriesSlug: string; issueSlug: string }
  | { variant: "series"; seriesSlug: string };

export function AppearancesTab(props: Props) {
  // Both hooks are declared so the hook order is stable; the inactive one is
  // disabled via an empty slug (the hooks gate on `enabled: !!slug`).
  const issueQ = useIssueAppearances(
    props.variant === "issue" ? props.seriesSlug : "",
    props.variant === "issue" ? props.issueSlug : "",
  );
  const seriesQ = useSeriesAppearances(
    props.variant === "series" ? props.seriesSlug : "",
  );
  const q = props.variant === "issue" ? issueQ : seriesQ;

  if (q.isLoading) {
    return (
      <div className="text-muted-foreground flex items-center gap-2 py-8 text-sm">
        <Loader2 className="h-4 w-4 animate-spin" />
        Loading appearances…
      </div>
    );
  }
  if (q.isError || !q.data) {
    return (
      <p className="text-muted-foreground py-8 text-sm">
        Couldn&rsquo;t load where this {props.variant} appears.
      </p>
    );
  }

  const { reading_lists, collections, arcs } = q.data;
  const total = reading_lists.length + collections.length + arcs.length;
  if (total === 0) {
    return (
      <p className="text-muted-foreground py-8 text-sm">
        This {props.variant} isn&rsquo;t in any of your reading lists or
        collections, or any story arc yet.
      </p>
    );
  }

  return (
    <div className="space-y-6">
      <Section
        title="Reading lists"
        icon={<BookMarked className="h-4 w-4" />}
        items={reading_lists}
        variant={props.variant}
      />
      <Section
        title="Collections"
        icon={<Library className="h-4 w-4" />}
        items={collections}
        variant={props.variant}
      />
      <Section
        title="Story arcs"
        icon={<Layers className="h-4 w-4" />}
        items={arcs}
        variant={props.variant}
      />
    </div>
  );
}

function Section({
  title,
  icon,
  items,
  variant,
}: {
  title: string;
  icon: React.ReactNode;
  items: AppearanceView[];
  variant: "issue" | "series";
}) {
  if (items.length === 0) return null;
  return (
    <section className="space-y-2">
      <h3 className="text-foreground flex items-center gap-2 text-sm font-semibold">
        <span className="text-muted-foreground">{icon}</span>
        {title}
        <span className="text-muted-foreground text-xs font-normal">
          {items.length}
        </span>
      </h3>
      <ul className="border-border/60 divide-border/60 divide-y overflow-hidden rounded-md border">
        {items.map((item) => (
          <AppearanceRow
            key={`${item.kind}:${item.id}`}
            item={item}
            variant={variant}
          />
        ))}
      </ul>
    </section>
  );
}

function AppearanceRow({
  item,
  variant,
}: {
  item: AppearanceView;
  variant: "issue" | "series";
}) {
  const meta = metaLabel(item, variant);
  // Reading lists and collections are saved views → navigable. Story arcs
  // have no detail route yet, so they stay as a static row.
  const navigable = item.kind === "cbl" || item.kind === "collection";

  const inner = (
    <>
      <span className="text-foreground truncate">{item.name}</span>
      <span className="text-muted-foreground ml-auto shrink-0 text-xs">
        {meta}
      </span>
    </>
  );

  if (navigable) {
    return (
      <li>
        <Link
          href={`/views/${item.id}`}
          className="hover:bg-muted/40 flex items-center gap-3 px-3 py-2 text-sm transition-colors"
        >
          {inner}
        </Link>
      </li>
    );
  }
  return (
    <li className="flex items-center gap-3 px-3 py-2 text-sm">
      {inner}
      {!meta && (
        <Badge variant="outline" className="ml-auto font-normal">
          arc
        </Badge>
      )}
    </li>
  );
}

/** Right-aligned context: the issue's position within a list/arc, or, on the
 *  series side, how many of the series' issues a container holds. */
function metaLabel(item: AppearanceView, variant: "issue" | "series"): string {
  if (variant === "issue") {
    return item.position != null ? `#${item.position + 1}` : "";
  }
  const n = item.issue_count ?? 0;
  if (n <= 0) return "";
  return n === 1 ? "1 issue" : `${n} issues`;
}
