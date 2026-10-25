import { signInUrl } from "@/lib/api/sign-in-url";
import { ArrowLeft, ChevronRight } from "lucide-react";
import Link from "next/link";
import { notFound, redirect } from "next/navigation";

import { PageHeader } from "@/components/admin/PageHeader";
import { HorizontalScrollRail } from "@/components/library/HorizontalScrollRail";
import { SeriesCard } from "@/components/library/SeriesCard";
import { apiGet, ApiError } from "@/lib/api/fetch";
import type { CreatorDetailView, CreatorRoleRail } from "@/lib/api/types";

/** Hard cap on cards mounted per role rail on the overview page.
 *  The rail is a horizontal scroller, so the cap never shows as a
 *  ragged edge — it only bounds how far the strip scrolls before the
 *  trailing "View all" tile (and the header link) hand off to the
 *  per-role grid via `?role=`. It exists because the detail payload
 *  carries every series per role as a full `SeriesView`; a prolific
 *  writer would otherwise mount hundreds of cards. */
const RAIL_CAP = 24;

/** Fixed per-card width inside the overview rails. Matches the
 *  drill-in grid's minimum column so a cover is the same size on both
 *  layouts; the rail shows however many fit the viewport and scrolls
 *  for the rest, which is what keeps it fluid at every width. */
const CARD_WIDTH_PX = 180;

/** Canonical role list for the chip row in the header. Mirrors the
 *  backend's ROLE_ORDER so order stays stable between the overview
 *  page and the per-role drill-in. */
const CANONICAL_ROLES: readonly string[] = [
  "writer",
  "penciller",
  "inker",
  "colorist",
  "letterer",
  "cover_artist",
  "editor",
  "translator",
];

/** Search-improvements M8: creator detail page.
 *
 *  Two layouts share the same fetch:
 *  - **Default** (no `?role=`): the overview. One section per role
 *    the creator held, truncated to `RAIL_CAP` cards each, with a
 *    "View all <N>" link in the section header that deep-links to
 *    the per-role drill-in.
 *  - **Per-role drill-in** (`?role=writer`): full grid of every
 *    series the creator touched in that one role, with a back link
 *    to the overview.
 *
 *  Server-rendered — `<SeriesCard>` already owns the interactive
 *  surface.
 */
export default async function CreatorPage({
  params,
  searchParams,
}: {
  params: Promise<{ slug: string }>;
  searchParams: Promise<{ role?: string }>;
}) {
  const { slug } = await params;
  const { role: activeRoleRaw } = await searchParams;
  let detail: CreatorDetailView;
  try {
    detail = await apiGet<CreatorDetailView>(
      `/creators/${encodeURIComponent(slug)}`,
    );
  } catch (e) {
    if (e instanceof ApiError) {
      if (e.status === 401) redirect(await signInUrl());
      if (e.status === 404) notFound();
    }
    throw e;
  }

  // Resolve `?role=` against the rails the creator actually has, so
  // a stale URL (link from a year ago, creator's role mix has
  // changed since) falls back to the overview rather than 404ing.
  const activeRole =
    activeRoleRaw && detail.rails.some((r) => r.role === activeRoleRaw)
      ? activeRoleRaw
      : null;
  const activeRail = activeRole
    ? (detail.rails.find((r) => r.role === activeRole) ?? null)
    : null;

  return (
    <div className="space-y-8">
      <CreatorHeader
        detail={detail}
        activeRole={activeRole}
        activeRailCount={activeRail?.series.length ?? 0}
      />
      {detail.rails.length === 0 ? (
        <p className="text-muted-foreground text-sm">
          This creator has no visible credits in any library you can access.
        </p>
      ) : activeRail ? (
        <SingleRoleGrid rail={activeRail} />
      ) : (
        detail.rails.map((rail) => (
          <RoleRail key={rail.role} rail={rail} creatorSlug={detail.slug} />
        ))
      )}
    </div>
  );
}

/** Page header — title + a one-line summary that flips between
 *  overview ("N series · roles…") and drill-in ("Series this creator
 *  wrote · Back to overview"). Role chips on the overview are
 *  clickable so the user can jump straight into a role section
 *  without scrolling. */
function CreatorHeader({
  detail,
  activeRole,
  activeRailCount,
}: {
  detail: CreatorDetailView;
  activeRole: string | null;
  activeRailCount: number;
}) {
  if (activeRole) {
    return (
      <div className="space-y-2">
        <Link
          href={`/creators/${encodeURIComponent(detail.slug)}`}
          className="text-muted-foreground hover:text-foreground inline-flex items-center gap-1 text-xs"
        >
          <ArrowLeft className="size-3" aria-hidden="true" />
          Back to overview
        </Link>
        <PageHeader
          title={detail.name}
          description={`${activeRailCount} series · as ${formatRole(activeRole).toLowerCase()}`}
        />
      </div>
    );
  }
  if (detail.rails.length === 0) {
    return (
      <PageHeader
        title={detail.name}
        description="No visible credits in your accessible libraries."
      />
    );
  }
  const total = detail.credit_count;
  return (
    <PageHeader
      title={detail.name}
      description={`${total} series across ${detail.rails.length} ${detail.rails.length === 1 ? "role" : "roles"}`}
      actions={
        <div className="flex flex-wrap items-center gap-1.5">
          {sortRoles(detail.rails.map((r) => r.role)).map((role) => {
            const rail = detail.rails.find((r) => r.role === role);
            const count = rail?.series.length ?? 0;
            return (
              <Link
                key={role}
                href={`/creators/${encodeURIComponent(detail.slug)}?role=${role}`}
                className="border-border bg-muted/40 hover:bg-muted text-foreground inline-flex items-center gap-1.5 rounded-full border px-2.5 py-1 text-xs transition-colors"
              >
                <span>{formatRole(role)}</span>
                <span className="text-muted-foreground tabular-nums">
                  {count}
                </span>
              </Link>
            );
          })}
        </div>
      }
    />
  );
}

/** One role section on the overview: a horizontal `HorizontalScrollRail`
 *  of fixed-width series cards, the same strip the home page, search
 *  categories and the series Related tab use. A fixed card count in an
 *  `auto-fill` grid (the previous layout) left a ragged last row at
 *  every column count other than 4 or 6; a scroller shows whatever
 *  fits and scrolls for the rest. Mounts at most RAIL_CAP cards; when
 *  the role has more, a header "View all <N>" link and the rail's
 *  trailing tile both land on the per-role drill-in (the search page
 *  keeps both too — the tile alone is easy to miss past the first
 *  screen of cards, especially on touch). */
function RoleRail({
  rail,
  creatorSlug,
}: {
  rail: CreatorRoleRail;
  creatorSlug: string;
}) {
  const shown = rail.series.slice(0, RAIL_CAP);
  const truncated = shown.length < rail.series.length;
  const viewAllHref = `/creators/${encodeURIComponent(creatorSlug)}?role=${rail.role}`;
  const roleLabel = formatRole(rail.role).toLowerCase();
  return (
    <section className="space-y-3" data-role={rail.role}>
      <header className="flex flex-wrap items-center gap-2">
        <h2 className="text-base font-semibold tracking-tight">
          As {formatRole(rail.role).toLowerCase()}
          <span className="text-muted-foreground ml-2 text-xs font-normal">
            {rail.series.length} series
          </span>
        </h2>
        {truncated ? (
          <Link
            href={viewAllHref}
            className="text-muted-foreground hover:text-foreground ml-auto inline-flex items-center gap-1 text-xs font-medium"
          >
            View all {rail.series.length}
            <ChevronRight className="size-3" aria-hidden="true" />
          </Link>
        ) : null}
      </header>
      <HorizontalScrollRail
        as="ul"
        trackLabel={`Series as ${roleLabel}`}
        itemWidthPx={CARD_WIDTH_PX}
        viewAllHref={truncated ? viewAllHref : undefined}
        viewAllLabel={`View all ${rail.series.length} as ${roleLabel}`}
      >
        {shown.map((s) => (
          <li
            key={s.id}
            className="shrink-0"
            style={{ width: `${CARD_WIDTH_PX}px` }}
          >
            <SeriesCard series={s} size="md" />
          </li>
        ))}
      </HorizontalScrollRail>
    </section>
  );
}

/** Drill-in view: every series the creator touched in one role, no
 *  per-role cap, laid out as a fluid cover grid. */
function SingleRoleGrid({ rail }: { rail: CreatorRoleRail }) {
  return (
    <section data-role={rail.role}>
      <SeriesGrid series={rail.series} />
    </section>
  );
}

function SeriesGrid({ series }: { series: CreatorRoleRail["series"] }) {
  return (
    <ul
      role="list"
      className="grid gap-4"
      // `min(…, calc(50% - 0.5rem))` keeps two columns on a phone — the
      // guard every other cover grid uses. A bare 180px minimum
      // collapsed to one full-width column below ~376px.
      style={{
        gridTemplateColumns: `repeat(auto-fill, minmax(min(${CARD_WIDTH_PX}px, calc(50% - 0.5rem)), 1fr))`,
      }}
    >
      {series.map((s) => (
        <li key={s.id}>
          <SeriesCard series={s} size="md" />
        </li>
      ))}
    </ul>
  );
}

/** Title-case a role token ("cover_artist" → "Cover artist"). */
function formatRole(role: string): string {
  return role
    .split("_")
    .map((s, i) =>
      s.length === 0
        ? s
        : i === 0
          ? s[0]!.toUpperCase() + s.slice(1)
          : s.toLowerCase(),
    )
    .join(" ");
}

/** Sort roles by canonical order, with unknown roles falling through
 *  alphabetically at the end. Used to keep the chip row stable
 *  between the overview header and the per-role drill-in. */
function sortRoles(roles: readonly string[]): string[] {
  return [...roles].sort((a, b) => {
    const ai = CANONICAL_ROLES.indexOf(a);
    const bi = CANONICAL_ROLES.indexOf(b);
    if (ai === -1 && bi === -1) return a.localeCompare(b);
    if (ai === -1) return 1;
    if (bi === -1) return -1;
    return ai - bi;
  });
}
