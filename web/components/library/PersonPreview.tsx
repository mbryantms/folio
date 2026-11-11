"use client";

import { useQuery } from "@tanstack/react-query";
import Link from "next/link";
import { useState } from "react";

import { Cover } from "@/components/Cover";
import {
  HoverCard,
  HoverCardContent,
  HoverCardTrigger,
} from "@/components/folio/hover-card";
import { jsonFetch, queryKeys } from "@/lib/api/queries";
import type { components } from "@/lib/api/types.generated";
import { seriesUrl } from "@/lib/urls";
import { useVisualEffect } from "@/lib/visual-effects";
import { RailError } from "@/components/saved-views/RailError";

type Schemas = components["schemas"];
type CreatorDetail = Schemas["CreatorDetailView"];
type EntityDetail = Schemas["EntityDetailView"];
type SeriesList = Schemas["SeriesListView"];
type Series = Schemas["SeriesView"];

/** What a chip links to, when it is something we can preview. */
export type PreviewTarget =
  | { kind: "creators"; slug: string }
  | { kind: "characters" | "teams"; slug: string };

/** `/creators/x`, `/characters/x`, `/teams/x` → a preview target. */
export function previewTargetFor(href: string): PreviewTarget | null {
  const m = href.match(/^\/(creators|characters|teams)\/([^/?#]+)$/);
  if (!m?.[1] || !m[2]) return null;
  return {
    kind: m[1] as PreviewTarget["kind"],
    slug: decodeURIComponent(m[2]),
  };
}

/** Two-letter monogram: first letters of the first and last words. */
export function monogram(name: string): string {
  const words = name.trim().split(/\s+/).filter(Boolean);
  if (words.length === 0) return "?";
  const first = words[0]?.[0] ?? "";
  const last = words.length > 1 ? (words[words.length - 1]?.[0] ?? "") : "";
  return (first + last).toUpperCase();
}

/** Stable hue for a name, so a person keeps their colour everywhere. */
export function nameHue(name: string): number {
  let h = 2166136261;
  for (const c of name) h = Math.imul(h ^ c.charCodeAt(0), 16777619);
  return (h >>> 0) % 360;
}

const ROLE_LABEL: Record<string, string> = {
  writer: "Writer",
  penciller: "Penciller",
  inker: "Inker",
  colorist: "Colorist",
  letterer: "Letterer",
  cover_artist: "Cover artist",
  editor: "Editor",
  translator: "Translator",
};

/**
 * "Creator and character previews" (folio-flair-1.0, optional, on by
 * default). Wraps a creator / character / team chip link: resting the
 * pointer on it (or focusing it with the keyboard) opens a card with who
 * they are, how much of them is in the library, and a few of their series.
 * Data is fetched only when the card opens. Touch keeps the plain link
 * (a tap opens the full page). With the effect off, or for anything we
 * can't preview, the chip renders unchanged.
 */
export function PersonPreview({
  href,
  children,
}: {
  href: string;
  children: React.ReactElement;
}) {
  const on = useVisualEffect("hover_cards");
  const [open, setOpen] = useState(false);
  const target = previewTargetFor(href);
  if (!on || !target) return children;
  return (
    <HoverCard openDelay={250} closeDelay={120} onOpenChange={setOpen}>
      <HoverCardTrigger asChild>{children}</HoverCardTrigger>
      <HoverCardContent align="start" className="w-80">
        {open && <PreviewBody target={target} href={href} />}
      </HoverCardContent>
    </HoverCard>
  );
}

function PreviewBody({
  target,
  href,
}: {
  target: PreviewTarget;
  href: string;
}) {
  const q = useQuery({
    queryKey: queryKeys.personPreview(target.kind, target.slug),
    staleTime: 5 * 60_000,
    queryFn: async () => {
      const slug = encodeURIComponent(target.slug);
      if (target.kind === "creators") {
        const d = await jsonFetch<CreatorDetail>(`/creators/${slug}`);
        const seen = new Set<string>();
        const series: Series[] = [];
        for (const rail of d.rails) {
          for (const s of rail.series) {
            if (!seen.has(s.id)) {
              seen.add(s.id);
              series.push(s);
            }
          }
        }
        return {
          name: d.name,
          image: null as string | null,
          subtitle: d.roles.map((r) => ROLE_LABEL[r] ?? r).join(", "),
          stats: [
            { value: d.credit_count, label: "credits" },
            { value: series.length, label: "series here" },
          ],
          blurb: null as string | null,
          series: series.slice(0, 3),
        };
      }
      const [d, list] = await Promise.all([
        jsonFetch<EntityDetail>(`/${target.kind}/${slug}`),
        jsonFetch<SeriesList>(`/${target.kind}/${slug}/series?limit=3`),
      ]);
      return {
        name: d.name,
        image: d.image_url ?? null,
        subtitle:
          d.real_name && d.real_name !== d.name
            ? d.real_name
            : target.kind === "teams"
              ? "Team"
              : "Character",
        stats: [
          { value: d.issue_count, label: "issues" },
          { value: d.series_count, label: "series" },
        ],
        blurb: d.description ?? null,
        series: list.items.slice(0, 3),
      };
    },
  });

  if (q.isPending) {
    return (
      <div className="flex items-center gap-3" aria-busy="true">
        <div className="bg-muted size-12 animate-pulse rounded-full" />
        <div className="flex-1 space-y-2">
          <div className="bg-muted h-3 w-2/3 animate-pulse rounded" />
          <div className="bg-muted h-2.5 w-1/3 animate-pulse rounded" />
        </div>
      </div>
    );
  }
  if (q.isError || !q.data) {
    return (
      <RailError
        message="Preview unavailable."
        onRetry={() => void q.refetch()}
      />
    );
  }
  const p = q.data;
  const hue = nameHue(p.name);
  return (
    <div className="space-y-3">
      <div className="flex items-center gap-3">
        {p.image ? (
          // eslint-disable-next-line @next/next/no-img-element
          <img
            src={p.image}
            alt=""
            className="size-12 shrink-0 rounded-full object-cover"
          />
        ) : (
          <span
            aria-hidden
            className="grid size-12 shrink-0 place-items-center rounded-full text-sm font-semibold text-white"
            style={{
              background: `linear-gradient(135deg, oklch(0.62 0.15 ${hue}), oklch(0.48 0.14 ${(hue + 40) % 360}))`,
            }}
          >
            {monogram(p.name)}
          </span>
        )}
        <div className="min-w-0">
          <Link
            href={href}
            className="block truncate font-semibold hover:underline"
          >
            {p.name}
          </Link>
          {p.subtitle && (
            <p className="text-muted-foreground truncate text-xs">
              {p.subtitle}
            </p>
          )}
        </div>
      </div>
      <dl className="flex gap-5">
        {p.stats.map((s) => (
          <div key={s.label}>
            <dt className="sr-only">{s.label}</dt>
            <dd className="text-base font-semibold tabular-nums">
              {s.value.toLocaleString()}
              <span className="text-muted-foreground ml-1 text-xs font-normal">
                {s.label}
              </span>
            </dd>
          </div>
        ))}
      </dl>
      {p.blurb && (
        <p className="text-muted-foreground line-clamp-3 text-xs">{p.blurb}</p>
      )}
      {p.series.length > 0 && (
        <div className="grid grid-cols-3 gap-2">
          {p.series.map((s) => (
            <Link
              key={s.id}
              href={seriesUrl(s)}
              className="group min-w-0 space-y-1"
            >
              <Cover
                src={s.cover_url}
                alt={s.name}
                fallback={s.publisher ?? s.name}
                className="w-full transition group-hover:brightness-110"
              />
              <span className="text-muted-foreground block truncate text-[11px]">
                {s.name}
              </span>
            </Link>
          ))}
        </div>
      )}
    </div>
  );
}
