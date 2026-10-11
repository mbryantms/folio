"use client";

import { ExternalLink, RefreshCw } from "lucide-react";
import Link from "next/link";
import * as React from "react";

import { ProviderCoverImage } from "@/components/library/ProviderCoverImage";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import type { EntityKindPath } from "@/lib/api/queries";
import type {
  EntityDetailView,
  EntityProfileFacts,
  EntityProfileSection,
} from "@/lib/api/types";

/** Provider labels for the "filled from" footer. */
const SOURCE_LABEL: Record<string, string> = {
  metron: "Metron",
  comicvine: "ComicVine",
  gcd: "Grand Comics Database",
};

export function sourceLabel(source: string): string {
  return SOURCE_LABEL[source] ?? source;
}

/** "ComicVine lists 25,186 issues · you own 6,147 of them (24%)" for the
 *  provider with the largest count, or `null` when no provider reported
 *  one. Pure so the tests pin the rounding. */
export function coverageLine(
  counts: Record<string, number>,
  owned: number,
): string | null {
  const entries = Object.entries(counts).filter(([, n]) => n > 0);
  if (entries.length === 0) return null;
  const [source, total] = entries.sort((a, b) => b[1] - a[1])[0]!;
  const pct = Math.min(100, Math.round((owned / total) * 100));
  return `${sourceLabel(source)} lists ${total.toLocaleString()} issues · you own ${owned.toLocaleString()} of them (${pct}%)`;
}

/** Facts worth a row in the grid, in display order; empty values drop. */
export function factRows(
  facts: EntityProfileFacts,
  kind: EntityKindPath,
): Array<{ label: string; value: string }> {
  const rows: Array<{ label: string; value: string }> = [];
  if (facts.real_name)
    rows.push({ label: "Real name", value: facts.real_name });
  if (facts.creators?.length)
    rows.push({ label: "Created by", value: facts.creators.join(", ") });
  if (facts.first_appearance)
    rows.push({
      label: "First appearance",
      value: facts.first_appearance.label,
    });
  if (facts.publisher)
    rows.push({ label: "Publisher", value: facts.publisher });
  if (facts.gender) rows.push({ label: "Gender", value: facts.gender });
  if (facts.birthday) rows.push({ label: "Birthday", value: facts.birthday });
  if (kind === "teams" && facts.members?.length)
    rows.push({
      label: "Members",
      value: `${facts.members.length} on record`,
    });
  return rows;
}

function ChipRow({
  label,
  items,
  cap = 16,
}: {
  label: string;
  items: string[] | undefined;
  cap?: number;
}) {
  const [expanded, setExpanded] = React.useState(false);
  if (!items?.length) return null;
  const shown = expanded ? items : items.slice(0, cap);
  const hidden = items.length - shown.length;
  return (
    <div className="space-y-1.5">
      <p className="text-muted-foreground text-xs font-semibold tracking-wider uppercase">
        {label}
      </p>
      <div className="flex flex-wrap items-center gap-1.5">
        {shown.map((item) => (
          <Badge key={item} variant="secondary" className="font-normal">
            {item}
          </Badge>
        ))}
        {hidden > 0 ? (
          <button
            type="button"
            onClick={() => setExpanded(true)}
            className="text-muted-foreground hover:text-foreground text-xs underline-offset-2 hover:underline"
          >
            + {hidden} more
          </button>
        ) : null}
      </div>
    </div>
  );
}

/** The provider-filled block of an entity landing page: image, summary,
 *  fact sheet, alias / power / team / universe chips, and the "filled
 *  from" footer with the appearance-coverage line. Renders the pending
 *  / failed / unavailable states too, so the page always says where
 *  the profile stands. */
export function EntityProfile({
  kind,
  detail,
  isAdmin,
  onRefresh,
  refreshing,
}: {
  kind: EntityKindPath;
  detail: EntityDetailView;
  isAdmin: boolean;
  onRefresh?: () => void;
  refreshing?: boolean;
}) {
  const profile = detail.profile ?? null;
  const summary = profile?.summary ?? detail.description ?? null;
  const facts = profile?.facts;
  const rows = facts ? factRows(facts, kind) : [];
  const coverage = profile
    ? coverageLine(profile.provider_counts, detail.issue_count)
    : null;
  const hasBody =
    !!detail.image_url ||
    !!summary ||
    detail.aliases.length > 0 ||
    rows.length > 0;

  return (
    <section className="bg-card border-border rounded-lg border p-4 sm:p-6">
      {hasBody ? (
        <div className="flex flex-col gap-5 sm:flex-row">
          {detail.image_url ? (
            // Provider-hosted image; not routed through next/image so the
            // CSP img-src policy (not a loader allowlist) governs it.
            <ProviderCoverImage
              src={detail.image_url}
              alt=""
              className="bg-muted h-64 w-44 shrink-0 self-start rounded-md object-cover"
              placeholderClassName="hidden"
            />
          ) : null}
          <div className="min-w-0 flex-1 space-y-4">
            {summary ? (
              <p className="text-foreground/90 max-w-prose text-[15px] leading-relaxed whitespace-pre-line">
                {summary}
              </p>
            ) : null}
            {rows.length > 0 ? (
              <dl className="grid grid-cols-1 gap-x-6 gap-y-3 text-sm sm:grid-cols-2 lg:grid-cols-3">
                {rows.map((r) => (
                  <div key={r.label}>
                    <dt className="text-muted-foreground text-xs font-semibold tracking-wider uppercase">
                      {r.label}
                    </dt>
                    <dd className="mt-0.5">{r.value}</dd>
                  </div>
                ))}
              </dl>
            ) : null}
            <ChipRow label="Also known as" items={detail.aliases} />
            <ChipRow label="Powers" items={facts?.powers} />
            <ChipRow label="Teams" items={facts?.teams} cap={12} />
            <ChipRow label="Universes" items={facts?.universes} />
            {kind === "teams" ? (
              <ChipRow label="Members" items={facts?.members} cap={20} />
            ) : null}
          </div>
        </div>
      ) : null}
      <ProfileFooter
        detail={detail}
        coverage={coverage}
        isAdmin={isAdmin}
        onRefresh={onRefresh}
        refreshing={refreshing}
        hasBody={hasBody}
      />
    </section>
  );
}

function ProfileFooter({
  detail,
  coverage,
  isAdmin,
  onRefresh,
  refreshing,
  hasBody,
}: {
  detail: EntityDetailView;
  coverage: string | null;
  isAdmin: boolean;
  onRefresh?: () => void;
  refreshing?: boolean;
  hasBody: boolean;
}) {
  const sources = detail.profile?.sources ?? [];
  let status: React.ReactNode;
  switch (detail.enrichment) {
    case "pending":
      status = (
        <span className="inline-flex items-center gap-1.5">
          <RefreshCw aria-hidden="true" className="size-3.5 animate-spin" />
          Fetching details from your metadata providers…
        </span>
      );
      break;
    case "failed":
      status = (
        <span>
          Couldn&apos;t fill this page.
          {detail.enrichment_error ? ` ${detail.enrichment_error}.` : ""}
        </span>
      );
      break;
    case "done":
      status = (
        <span>
          Filled from {sources.map(sourceLabel).join(" + ")}
          {detail.profile?.fetched_at
            ? ` · ${new Date(detail.profile.fetched_at).toLocaleDateString()}`
            : ""}
          {detail.enrichment_error ? ` · ${detail.enrichment_error}` : ""}
        </span>
      );
      break;
    default:
      status = hasBody ? null : (
        <span>
          Add a metadata provider under Admin → Metadata to fill this page.
        </span>
      );
  }
  const links = detail.links ?? [];
  if (!status && !coverage && links.length === 0 && !isAdmin) {
    return null;
  }
  return (
    <div
      className={
        "text-muted-foreground flex flex-wrap items-center gap-x-4 gap-y-2 text-xs" +
        (hasBody ? " border-border mt-5 border-t pt-4" : "")
      }
    >
      {status}
      {coverage ? <span className="text-foreground/80">{coverage}</span> : null}
      {links.map((l) =>
        l.url ? (
          <Link
            key={l.source}
            href={l.url}
            target="_blank"
            rel="noreferrer noopener"
            className="hover:text-foreground inline-flex items-center gap-1"
          >
            {l.label}
            <ExternalLink aria-hidden="true" className="size-3" />
          </Link>
        ) : (
          <span key={l.source}>
            {l.label} #{l.external_id}
          </span>
        ),
      )}
      {isAdmin && detail.enrichment !== "unavailable" && onRefresh ? (
        <Button
          type="button"
          variant="ghost"
          size="sm"
          className="ml-auto h-7 px-2 text-xs"
          onClick={onRefresh}
          disabled={refreshing || detail.enrichment === "pending"}
        >
          <RefreshCw aria-hidden="true" className="size-3.5" />
          Refresh
        </Button>
      ) : null}
    </div>
  );
}

/** The wiki sections ComicVine's description splits into: the lead and
 *  first section open, the rest as heading chips that expand in place. */
export function EntitySections({
  sections,
}: {
  sections: EntityProfileSection[];
}) {
  const [open, setOpen] = React.useState<number | null>(null);
  const lead = sections.filter((s) => s.level === 0);
  const rest = sections.filter((s) => s.level !== 0);
  if (sections.length === 0) return null;
  const first = rest[0];
  const expanded = open != null ? rest[open] : null;
  const chips = rest.slice(1);
  return (
    <section className="bg-card border-border space-y-4 rounded-lg border p-4 sm:p-6">
      <div className="flex items-baseline justify-between gap-3">
        <h2 className="text-lg font-semibold">
          {first ? first.heading : "About"}
        </h2>
        {rest.length > 1 ? (
          <span className="text-muted-foreground text-xs">
            {rest.length} sections
          </span>
        ) : null}
      </div>
      <Paragraphs items={[...lead, ...(first ? [first] : [])]} />
      {chips.length > 0 ? (
        <div className="border-border space-y-4 border-t pt-4">
          <div className="flex flex-wrap items-center gap-1.5">
            <span className="text-muted-foreground mr-1 text-xs font-semibold tracking-wider uppercase">
              More
            </span>
            {chips.map((s, i) => {
              const idx = i + 1;
              const active = open === idx;
              return (
                <button
                  key={`${s.heading}-${idx}`}
                  type="button"
                  aria-pressed={active}
                  onClick={() => setOpen(active ? null : idx)}
                  className={
                    "rounded-full px-2.5 py-1 text-xs transition-colors " +
                    (active
                      ? "bg-primary text-primary-foreground"
                      : "bg-muted text-foreground/90 hover:bg-muted/70")
                  }
                >
                  {s.heading}
                </button>
              );
            })}
          </div>
          {expanded && open !== 0 ? (
            <div className="space-y-2">
              <h3 className="text-sm font-semibold">{expanded.heading}</h3>
              <Paragraphs items={[expanded]} />
            </div>
          ) : null}
        </div>
      ) : null}
    </section>
  );
}

function Paragraphs({ items }: { items: EntityProfileSection[] }) {
  return (
    <div className="max-w-prose space-y-3 text-sm leading-relaxed">
      {items.flatMap((s, si) =>
        s.paragraphs.map((p, pi) => (
          <p
            key={`${si}-${pi}`}
            className="text-foreground/90 whitespace-pre-line"
          >
            {p}
          </p>
        )),
      )}
    </div>
  );
}
