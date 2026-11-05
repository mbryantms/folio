"use client";

import {
  BookOpen,
  Building2,
  Clock3,
  Drama,
  Folder,
  LayoutDashboard,
  ListChecks,
  MapPin,
  Milestone,
  User,
  Users,
  type LucideIcon,
} from "lucide-react";
import type { ComponentType } from "react";

import type { MarkerRegion } from "@/lib/api/types";
import { highlightSegments } from "@/lib/search/highlight";
import type { RecentVisit, RecentVisitKind } from "@/lib/search/recent-visits";
import { renderSearchSnippet } from "@/lib/search/render-snippet";
import type { SearchCategory, SearchHit } from "@/lib/search/types";
import { cn } from "@/lib/utils";

/**
 * Row internals shared by the ⌘K dialog (`CommandItem`) and the desktop
 * topbar dropdown (`ResultRow`): a cover / avatar / icon tile, the title
 * with the typed text marked, and one meta line (plus the backend's
 * `<mark>` excerpt when the match was in the summary, so the row shows
 * *why* it matched). Also renders the "Recently visited" rows.
 */

const MARK_CLS =
  "[&_mark]:bg-primary/25 [&_mark]:text-current [&_mark]:rounded-sm [&_mark]:px-px";

/** People-ish categories render as a round avatar, everything else as a
 *  cover-shaped tile. */
const ROUND_HIT_KINDS: ReadonlySet<SearchCategory> = new Set([
  "people",
  "characters",
  "teams",
]);
const ROUND_VISIT_KINDS: ReadonlySet<RecentVisitKind> = new Set([
  "creator",
  "characters",
  "teams",
]);

const VISIT_ICON: Record<RecentVisitKind, LucideIcon> = {
  series: BookOpen,
  issue: BookOpen,
  creator: User,
  characters: Drama,
  teams: Users,
  locations: MapPin,
  arcs: Milestone,
  publishers: Building2,
  view: ListChecks,
  collection: Folder,
  page: LayoutDashboard,
};

/** Title text with every query term wrapped in `<mark>`. */
export function Highlight({ text, query }: { text: string; query: string }) {
  return (
    <>
      {highlightSegments(text, query).map((seg, i) =>
        seg.match ? <mark key={i}>{seg.text}</mark> : seg.text,
      )}
    </>
  );
}

function Thumb({
  src,
  region,
  icon: Icon = BookOpen,
  round = false,
}: {
  src?: string | null;
  region?: MarkerRegion | null;
  icon?: ComponentType<{ className?: string }>;
  round?: boolean;
}) {
  const box = round ? "size-9 self-center rounded-full" : "h-12 w-9 rounded";
  const cls = cn("border-border bg-muted shrink-0 overflow-hidden border", box);
  if (src && region) {
    const scaleW = Math.min(100, 100 / Math.max(region.w, 1));
    const scaleH = Math.min(100, 100 / Math.max(region.h, 1));
    return (
      <div aria-hidden="true" className={cn(cls, "relative")}>
        {/* eslint-disable-next-line @next/next/no-img-element */}
        <img
          src={src}
          alt=""
          loading="lazy"
          decoding="async"
          className="max-w-none"
          style={{
            position: "absolute",
            width: `${scaleW * 100}%`,
            height: `${scaleH * 100}%`,
            left: `${-region.x * scaleW}%`,
            top: `${-region.y * scaleH}%`,
          }}
        />
      </div>
    );
  }
  if (src) {
    return (
      // Decorative: the title beside it already names the row.
      // eslint-disable-next-line @next/next/no-img-element
      <img
        src={src}
        alt=""
        loading="lazy"
        decoding="async"
        className={cn(cls, "object-cover")}
      />
    );
  }
  return (
    <div
      aria-hidden="true"
      className={cn(cls, "text-muted-foreground grid place-items-center")}
    >
      <Icon className="size-4" />
    </div>
  );
}

function RowText({
  title,
  query,
  meta,
  snippet,
}: {
  title: string;
  query: string;
  meta?: string | null;
  snippet?: string | null;
}) {
  return (
    <span className="min-w-0 flex-1">
      <span className={cn("block truncate font-medium", MARK_CLS)}>
        <Highlight text={title} query={query} />
      </span>
      {meta ? (
        <span className="block truncate text-xs opacity-70">{meta}</span>
      ) : null}
      {snippet ? (
        // Sanitised by `renderSearchSnippet` (only `<mark>` survives).
        <span
          className={cn("block truncate text-xs italic opacity-70", MARK_CLS)}
          dangerouslySetInnerHTML={{ __html: renderSearchSnippet(snippet) }}
        />
      ) : null}
    </span>
  );
}

/** One content hit: cover/avatar, highlighted title, meta line, and the
 *  summary excerpt when the backend sent one. */
export function SearchHitRowContent({
  hit,
  query,
}: {
  hit: SearchHit;
  query: string;
}) {
  return (
    <>
      <Thumb
        src={hit.thumbUrl}
        region={hit.region}
        icon={hit.icon}
        round={ROUND_HIT_KINDS.has(hit.kind)}
      />
      <RowText
        title={hit.title}
        query={query}
        meta={hit.subtitle}
        snippet={hit.snippet}
      />
    </>
  );
}

/** One "Recently visited" row. */
export function VisitRowContent({ visit }: { visit: RecentVisit }) {
  return (
    <>
      <Thumb
        src={visit.thumbUrl}
        icon={VISIT_ICON[visit.kind]}
        round={ROUND_VISIT_KINDS.has(visit.kind)}
      />
      <RowText title={visit.title} query="" meta={visit.subtitle} />
    </>
  );
}

/** Section header for the recent-visits group. Rendered outside cmdk's
 *  group heading (which cmdk marks `aria-hidden`) so "Clear" stays
 *  reachable for keyboard and screen-reader users. Matches the
 *  `RecentSearches` header. */
export function RecentVisitsHeader({ onClear }: { onClear: () => void }) {
  return (
    <div className="flex items-center justify-between px-3 pt-2 pb-1">
      <h3 className="text-muted-foreground inline-flex items-center gap-1.5 text-[11px] font-semibold tracking-wide uppercase">
        <Clock3 className="size-3" aria-hidden="true" />
        Recently visited
      </h3>
      <button
        type="button"
        onClick={onClear}
        className="text-muted-foreground hover:text-foreground text-[11px] font-medium"
      >
        Clear
      </button>
    </div>
  );
}
