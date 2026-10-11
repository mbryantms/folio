"use client";

import Link from "next/link";
import * as React from "react";

import { IssueCard, IssueCardSkeleton } from "@/components/library/IssueCard";
import { Skeleton } from "@/components/ui/skeleton";
import { useEntityInsights, type EntityKindPath } from "@/lib/api/queries";
import type { EntityInsightsView } from "@/lib/api/types";
import { entityUrl } from "@/lib/entities";
import { seriesUrl } from "@/lib/urls";

/** Bar heights (0–1) for the decade chart, scaled to the busiest decade
 *  and never below a hairline for a non-zero count. Pure for the tests. */
export function decadeBars(
  decades: EntityInsightsView["decades"],
): Array<{ decade: number; count: number; height: number }> {
  const max = Math.max(0, ...decades.map((d) => d.count));
  return decades.map((d) => ({
    decade: d.decade,
    count: d.count,
    height: max === 0 ? 0 : Math.max(0.02, d.count / max),
  }));
}

function n(v: number): string {
  return v.toLocaleString();
}

/** "In your library" — counts, first appearances you own, the decade
 *  spread, and the co-appearance lists, every number ACL-filtered by
 *  the server. */
export function EntityInsights({
  kind,
  slug,
}: {
  kind: EntityKindPath;
  slug: string;
}) {
  const query = useEntityInsights(kind, slug);
  if (query.isError) {
    return (
      <p className="text-muted-foreground text-sm">
        Couldn&apos;t load library statistics. Try refreshing.
      </p>
    );
  }
  const data = query.data;
  if (!data) return <InsightsSkeleton />;
  const span =
    data.first_year != null && data.last_year != null
      ? data.first_year === data.last_year
        ? String(data.first_year)
        : `${data.first_year} – ${data.last_year}`
      : "—";
  const lead = data.top_series[0];
  const publishers = data.publishers;
  return (
    <section className="space-y-4" aria-labelledby="entity-insights-heading">
      <h2 id="entity-insights-heading" className="text-lg font-semibold">
        In your library
      </h2>
      <div className="grid grid-cols-2 gap-3 lg:grid-cols-4">
        <Tile
          label="Issues"
          value={n(data.issues)}
          hint={
            publishers.length
              ? `${publishers[0]!.name} ${n(publishers[0]!.count)}${
                  publishers.length > 1
                    ? ` · ${publishers.length - 1} other publisher${publishers.length > 2 ? "s" : ""}`
                    : ""
                }`
              : undefined
          }
        />
        <Tile
          label="Series"
          value={n(data.series)}
          hint={lead ? `${lead.name} leads with ${n(lead.count)}` : undefined}
        />
        <Tile label="Span" value={span} />
        <Tile
          label="Read"
          value={n(data.read)}
          hint={`${n(Math.max(0, data.issues - data.read))} unread`}
        />
      </div>

      {data.earliest.length > 0 ? (
        <div className="bg-card border-border space-y-3 rounded-lg border p-4">
          <div className="flex items-baseline justify-between gap-3">
            <h3 className="text-sm font-semibold">
              {kind === "arcs"
                ? "Reading order starts with"
                : "Earliest appearances you own"}
            </h3>
          </div>
          <ul
            role="list"
            className="flex gap-3 overflow-x-auto pb-1"
            style={{ scrollbarWidth: "thin" }}
          >
            {data.earliest.map((issue) => (
              <li key={issue.id} className="w-[120px] shrink-0">
                <IssueCard issue={issue} />
              </li>
            ))}
          </ul>
        </div>
      ) : null}

      <div className="grid gap-4 lg:grid-cols-2">
        {data.decades.length > 0 ? (
          <div className="bg-card border-border space-y-3 rounded-lg border p-4">
            <h3 className="text-sm font-semibold">Issues by decade</h3>
            <DecadeChart decades={data.decades} />
          </div>
        ) : null}
        {data.characters.length > 0 ? (
          <div className="bg-card border-border space-y-3 rounded-lg border p-4">
            <h3 className="text-sm font-semibold">
              {kind === "characters" ? "Appears with" : "Characters"}
            </h3>
            <div className="flex flex-wrap gap-1.5">
              {data.characters.map((c) => (
                <CountChip
                  key={`${c.name}-${c.slug ?? ""}`}
                  name={c.name}
                  count={c.count}
                  href={c.slug ? entityUrl("characters", c.slug) : null}
                />
              ))}
            </div>
          </div>
        ) : null}
      </div>

      <div className="grid gap-4 md:grid-cols-2 xl:grid-cols-3">
        {data.top_series.length > 0 ? (
          <CountList
            title="Top series"
            rows={data.top_series.map((s) => ({
              key: s.slug,
              name: s.year != null ? `${s.name} (${s.year})` : s.name,
              href: seriesUrl({ slug: s.slug }),
              count: s.count,
            }))}
          />
        ) : null}
        {data.teams.length > 0 ? (
          <CountList
            title="Teams"
            rows={data.teams.map((t) => ({
              key: `${t.name}-${t.slug ?? ""}`,
              name: t.name,
              href: t.slug ? entityUrl("teams", t.slug) : null,
              count: t.count,
            }))}
          />
        ) : null}
        {data.locations.length > 0 ? (
          <CountList
            title="Locations"
            rows={data.locations.map((l) => ({
              key: `${l.name}-${l.slug ?? ""}`,
              name: l.name,
              href: l.slug ? entityUrl("locations", l.slug) : null,
              count: l.count,
            }))}
          />
        ) : null}
        {data.arcs.length > 0 ? (
          <CountList
            title="Story arcs"
            rows={data.arcs.map((a) => ({
              key: `${a.name}-${a.slug ?? ""}`,
              name: a.name,
              href: a.slug ? entityUrl("arcs", a.slug) : null,
              count: a.count,
            }))}
          />
        ) : null}
        {data.creators.length > 0 ? (
          <CountList
            title="Most frequent creators"
            rows={data.creators.map((c) => ({
              key: `${c.name}-${c.slug ?? ""}`,
              name: c.name,
              detail: c.roles.replaceAll("_", " "),
              href: c.slug ? `/creators/${encodeURIComponent(c.slug)}` : null,
              count: c.count,
            }))}
          />
        ) : null}
      </div>
    </section>
  );
}

function Tile({
  label,
  value,
  hint,
}: {
  label: string;
  value: string;
  hint?: string;
}) {
  return (
    <div className="bg-card border-border rounded-lg border px-4 py-3">
      <p className="text-muted-foreground text-xs font-semibold tracking-wider uppercase">
        {label}
      </p>
      <p className="mt-1 text-2xl font-semibold tabular-nums">{value}</p>
      {hint ? (
        <p className="text-muted-foreground mt-0.5 truncate text-xs">{hint}</p>
      ) : null}
    </div>
  );
}

function DecadeChart({ decades }: { decades: EntityInsightsView["decades"] }) {
  const bars = decadeBars(decades);
  return (
    <div
      className="flex h-36 items-end gap-2"
      role="img"
      aria-label={bars
        .map((b) => `${b.decade}s: ${n(b.count)} issues`)
        .join(", ")}
    >
      {bars.map((b) => (
        <div
          key={b.decade}
          className="flex min-w-0 flex-1 flex-col items-center gap-1"
        >
          <span className="text-foreground/90 text-[11px] tabular-nums">
            {n(b.count)}
          </span>
          <div
            className="bg-primary w-full rounded-t-sm"
            style={{ height: `${Math.round(b.height * 100)}px` }}
          />
          <span className="text-muted-foreground text-[11px]">
            {String(b.decade).slice(2)}s
          </span>
        </div>
      ))}
    </div>
  );
}

function CountChip({
  name,
  count,
  href,
}: {
  name: string;
  count: number;
  href: string | null;
}) {
  const body = (
    <>
      {name}
      <span className="text-muted-foreground text-xs tabular-nums">
        {n(count)}
      </span>
    </>
  );
  const cls =
    "bg-muted inline-flex items-center gap-1.5 rounded-full px-2.5 py-1 text-sm";
  return href ? (
    <Link href={href} className={cls + " hover:bg-muted/70"}>
      {body}
    </Link>
  ) : (
    <span className={cls}>{body}</span>
  );
}

function CountList({
  title,
  rows,
}: {
  title: string;
  rows: Array<{
    key: string;
    name: string;
    detail?: string;
    href: string | null;
    count: number;
  }>;
}) {
  return (
    <div className="bg-card border-border rounded-lg border p-4">
      <h3 className="mb-2 text-sm font-semibold">{title}</h3>
      <ul role="list" className="divide-border divide-y">
        {rows.map((r) => (
          <li
            key={r.key}
            className="flex items-baseline justify-between gap-3 py-1.5 text-sm"
          >
            <span className="min-w-0 truncate">
              {r.href ? (
                <Link href={r.href} className="hover:underline">
                  {r.name}
                </Link>
              ) : (
                r.name
              )}
              {r.detail ? (
                <span className="text-muted-foreground ml-1.5 text-xs">
                  {r.detail}
                </span>
              ) : null}
            </span>
            <span className="text-muted-foreground shrink-0 text-xs tabular-nums">
              {n(r.count)}
            </span>
          </li>
        ))}
      </ul>
    </div>
  );
}

function InsightsSkeleton() {
  return (
    <section className="space-y-4" aria-busy="true">
      <Skeleton className="h-6 w-40" />
      <div className="grid grid-cols-2 gap-3 lg:grid-cols-4">
        {Array.from({ length: 4 }).map((_, i) => (
          <Skeleton key={i} className="h-20" />
        ))}
      </div>
      <ul role="list" className="flex gap-3 overflow-hidden">
        {Array.from({ length: 6 }).map((_, i) => (
          <li key={i} className="w-[120px] shrink-0">
            <IssueCardSkeleton />
          </li>
        ))}
      </ul>
    </section>
  );
}
