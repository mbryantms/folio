/**
 * Recently-visited detail pages: the pure half (types, builders, list
 * logic). The `"use client"` store + hook live in `use-recent-visits.ts`;
 * this module stays React-free so server pages can import the builders
 * (`seriesVisit(series)` …) and hand the result to `<RecordVisit>`.
 *
 * Mirrors `use-recent-searches.ts`: a per-browser, most-recent-first
 * ring buffer in `localStorage`, deduped, capped, and tolerant of
 * corrupt storage.
 */

import type { EntityKindPath } from "@/lib/api/queries";
import type {
  CreatorDetailView,
  EntityDetailView,
  IssueDetailView,
  PageView,
  SavedViewView,
  SeriesView,
} from "@/lib/api/types";
import { entityUrl } from "@/lib/entities";
import { coverThumbUrl, issueUrl, seriesUrl } from "@/lib/urls";

/** Versioned like the recent-searches key so a schema change can
 *  re-namespace without tripping over stale entries. */
export const RECENT_VISITS_STORAGE_KEY = "folio.search.visits.v1";

/** Same cap as recent searches: enough for real recall without pushing
 *  the suggested commands off the bottom of the palette. */
export const RECENT_VISITS_MAX = 8;

export type RecentVisitKind =
  | "series"
  | "issue"
  | "creator"
  | EntityKindPath
  | "view"
  | "collection"
  | "page";

const VISIT_KINDS: ReadonlySet<string> = new Set<RecentVisitKind>([
  "series",
  "issue",
  "creator",
  "characters",
  "teams",
  "locations",
  "arcs",
  "publishers",
  "view",
  "collection",
  "page",
]);

export interface RecentVisit {
  kind: RecentVisitKind;
  /** Stable id of the visited thing; `kind` + `id` is the dedupe key. */
  id: string;
  title: string;
  /** One meta line: publisher · years · issues, series · #n, … */
  subtitle?: string | null;
  href: string;
  /** Cover (series / issue) or portrait (entity `image_url`). */
  thumbUrl?: string | null;
}

function sameVisit(a: RecentVisit, b: RecentVisit): boolean {
  return a.kind === b.kind && a.id === b.id;
}

/** Pure dedupe + cap: moves an existing entry (same kind + id) to the
 *  front with the fresh title / cover, so a renamed series or a newly
 *  fetched cover replaces the stale copy. Never mutates `prev`. */
export function appendVisit(
  prev: readonly RecentVisit[],
  next: RecentVisit,
): RecentVisit[] {
  if (!isRecentVisit(next)) return prev.slice();
  return [next, ...prev.filter((p) => !sameVisit(p, next))].slice(
    0,
    RECENT_VISITS_MAX,
  );
}

export function removeVisit(
  prev: readonly RecentVisit[],
  target: Pick<RecentVisit, "kind" | "id">,
): RecentVisit[] {
  return prev.filter((p) => !(p.kind === target.kind && p.id === target.id));
}

function isOptionalString(v: unknown): boolean {
  return v === undefined || v === null || typeof v === "string";
}

export function isRecentVisit(v: unknown): v is RecentVisit {
  if (!v || typeof v !== "object") return false;
  const o = v as Record<string, unknown>;
  return (
    typeof o.kind === "string" &&
    VISIT_KINDS.has(o.kind) &&
    typeof o.id === "string" &&
    o.id.length > 0 &&
    typeof o.title === "string" &&
    o.title.trim().length > 0 &&
    typeof o.href === "string" &&
    // Internal paths only: a tampered entry must not turn the palette
    // into an open redirect (`//evil.example`, `javascript:` …).
    o.href.startsWith("/") &&
    !o.href.startsWith("//") &&
    isOptionalString(o.subtitle) &&
    isOptionalString(o.thumbUrl)
  );
}

/** Parse the stored JSON. Anything malformed degrades to `[]`; bad
 *  entries inside a good array are dropped individually, then the list
 *  is re-deduped and capped. */
export function parseVisits(raw: string | null | undefined): RecentVisit[] {
  if (!raw) return [];
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return [];
  }
  if (!Array.isArray(parsed)) return [];
  const out: RecentVisit[] = [];
  for (const entry of parsed) {
    if (!isRecentVisit(entry)) continue;
    if (out.some((o) => sameVisit(o, entry))) continue;
    out.push({
      kind: entry.kind,
      id: entry.id,
      title: entry.title,
      subtitle: entry.subtitle ?? null,
      href: entry.href,
      thumbUrl: entry.thumbUrl ?? null,
    });
    if (out.length >= RECENT_VISITS_MAX) break;
  }
  return out;
}

// ───── Builders (server- and client-safe) ─────

function plural(n: number, one: string, many = `${one}s`): string {
  return `${n} ${n === 1 ? one : many}`;
}

function join(parts: ReadonlyArray<string | null | undefined | false>) {
  return parts.filter(Boolean).join(" · ") || null;
}

function yearSpan(
  from: number | null | undefined,
  to: number | null | undefined,
) {
  if (from == null) return to != null ? String(to) : null;
  if (to == null || to === from) return String(from);
  return `${from}–${to}`;
}

export function seriesVisit(s: SeriesView): RecentVisit {
  const count = s.issue_count ?? null;
  return {
    kind: "series",
    id: s.id,
    title: s.name,
    subtitle: join([
      s.publisher,
      yearSpan(s.earliest_year ?? s.year, s.latest_year),
      count != null ? plural(count, "issue") : null,
    ]),
    href: seriesUrl(s),
    thumbUrl: s.cover_url ?? null,
  };
}

export function issueVisit(i: IssueDetailView): RecentVisit {
  const number = i.number ? `#${i.number}` : null;
  return {
    kind: "issue",
    id: i.id,
    title:
      i.title || [i.series_name, number].filter(Boolean).join(" ") || "Issue",
    // An untitled issue's title is already "Series #n".
    subtitle: join([
      i.title ? i.series_name : "Issue",
      i.title ? number : null,
      i.year != null ? String(i.year) : null,
    ]),
    href: issueUrl(i),
    thumbUrl: i.state === "active" ? coverThumbUrl(i.id) : null,
  };
}

export function creatorVisit(c: CreatorDetailView): RecentVisit {
  const roles = c.roles
    .map((r) =>
      r
        .split("_")
        .map((w) => (w ? w[0]!.toUpperCase() + w.slice(1) : w))
        .join(" "),
    )
    .slice(0, 3);
  return {
    kind: "creator",
    id: c.id,
    title: c.name,
    subtitle: join([...roles, plural(c.credit_count, "credit")]),
    href: `/creators/${encodeURIComponent(c.slug)}`,
    thumbUrl: null,
  };
}

export function entityVisit(
  kind: EntityKindPath,
  e: EntityDetailView,
): RecentVisit {
  return {
    kind,
    id: e.id,
    title: e.name,
    subtitle: join([
      e.issue_count > 0 ? plural(e.issue_count, "issue") : null,
      `${e.series_count} series`,
    ]),
    href: entityUrl(kind, e.slug),
    thumbUrl: e.image_url ?? null,
  };
}

/** Saved views, reading lists and collections. System rails (Continue
 *  reading, On deck) are navigation, not destinations — skip them. */
export function savedViewVisit(v: SavedViewView): RecentVisit | null {
  if (v.kind === "system") return null;
  const isCollection = v.kind === "collection";
  return {
    kind: isCollection ? "collection" : "view",
    id: v.id,
    title: v.name,
    subtitle: isCollection
      ? "Collection"
      : v.kind === "cbl"
        ? "Reading list"
        : "Filter view",
    href:
      isCollection && v.system_key === "want_to_read"
        ? "/views/want-to-read"
        : `/views/${v.id}`,
    thumbUrl: null,
  };
}

export function pageVisit(
  p: Pick<PageView, "id" | "name" | "slug">,
): RecentVisit {
  return {
    kind: "page",
    id: p.id,
    title: p.name,
    subtitle: "Page",
    href: `/pages/${encodeURIComponent(p.slug)}`,
    thumbUrl: null,
  };
}
