import {
  BookOpen,
  Building2,
  FileStack,
  Clock,
  Calendar,
  Languages,
} from "lucide-react";
import Link from "next/link";
import { notFound, redirect } from "next/navigation";

import { Breadcrumbs } from "@/components/shell/Breadcrumbs";
import { Cover } from "@/components/Cover";
import { ChipList } from "@/components/library/ChipList";
import {
  DetailSection,
  DetailSummaryGrid,
  DetailSummaryItem,
} from "@/components/library/DetailSection";
import { Description } from "@/components/library/Description";
import { ExternalIdsCard } from "@/components/library/ExternalIdsCard";
import { MetadataGrid } from "@/components/library/MetadataGrid";
import { Stat } from "@/components/library/Stat";
import {
  StableTabsPanel,
  StableTabsPanelStack,
  StackedTabsPanel,
} from "@/components/library/StableTabsPanelStack";
import { UserRating } from "@/components/library/UserRating";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Progress } from "@/components/ui/progress";
import { SeriesActivityTab } from "@/components/activity/SeriesActivityTab";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { apiGet, ApiError } from "@/lib/api/fetch";
import type {
  IssueListView,
  LibraryView,
  SeriesResumeView,
  SeriesView,
} from "@/lib/api/types";
import {
  formatCompactPages,
  formatPageCount,
  formatPublicationStatus,
  formatReadingTime,
  formatReadingTimeCompact,
  formatRelativeDate,
} from "@/lib/format";
import { collectionStatus } from "@/lib/series-status";
import { type ReadState, readButtonLabel } from "@/lib/reading-state";
import { statusTone } from "@/lib/ui/status-tone";

import { readerUrl } from "@/lib/urls";

import { ProviderBadgesRow } from "@/components/library/ProviderBadgesRow";

import { CollectionTab } from "./CollectionTab";
import { IssuesPanel } from "./IssuesPanel";
import { SeriesActions } from "./SeriesActions";
import { SeriesSourcesFooter } from "./SeriesSourcesFooter";

export default async function SeriesPage({
  params,
  searchParams,
}: {
  params: Promise<{ slug: string }>;
  searchParams: Promise<{ q?: string }>;
}) {
  const { slug } = await params;
  const { q: initialQuery } = await searchParams;

  let series: SeriesView;
  let firstIssuePage: IssueListView;
  let resume: SeriesResumeView;
  let libraries: LibraryView[];
  try {
    // All depend only on the slug (or nothing) — fetch concurrently instead
    // of stacking sequential round-trips onto TTFB.
    // A bounded issue preview feeds "Read from beginning" and the activity
    // heatmap. The primary resume CTA comes from the server endpoint so
    // long series are not capped to this preview page. The libraries list
    // resolves the breadcrumb's library segment by `series.library_id`.
    [series, firstIssuePage, resume, libraries] = await Promise.all([
      apiGet<SeriesView>(`/series/${slug}`),
      apiGet<IssueListView>(`/series/${slug}/issues?limit=200`),
      apiGet<SeriesResumeView>(`/series/${slug}/resume`),
      apiGet<LibraryView[]>(`/libraries`),
    ]);
  } catch (e) {
    if (e instanceof ApiError) {
      if (e.status === 401) redirect(`/sign-in`);
      if (e.status === 404) notFound();
    }
    throw e;
  }

  // Breadcrumb library segment — `series` carries only `library_id`, so
  // resolve the name from the user's accessible libraries list.
  const library = libraries.find((l) => l.id === series.library_id) ?? null;

  const nextHref = resumeReaderHref(resume);
  const nextState = readStateFromResume(resume.state);
  // Same up-next target as the Read button, with incognito enabled — backs
  // the "Read incognito" actions-menu item.
  const readIncognitoHref = nextHref
    ? `${nextHref}${nextHref.includes("?") ? "&" : "?"}incognito=1`
    : null;
  // The first active issue is the "Read from beginning" target — independent
  // of the resume target so users can always restart from #1 even when
  // they're mid-way through a later issue.
  const firstIssue =
    firstIssuePage.items.find((i) => i.state === "active") ?? null;

  const status = formatPublicationStatus(series.status);
  const readingTime = formatReadingTime(series.total_page_count ?? 0);
  const releasedLabel = formatYearRange(
    series.earliest_year ?? series.year ?? null,
    series.latest_year ?? null,
  );
  const hasGenres = series.genres && series.genres.length > 0;
  const hasTags = series.tags && series.tags.length > 0;

  // Read-progress numbers come from the server now (`progress_summary`),
  // not the client-side first 100 issues — fixes the cap that pinned 192-
  // issue series at "0 / 100".
  const finishedCount = series.progress_summary?.finished ?? 0;
  const totalCount =
    series.progress_summary?.total ??
    series.issue_count ??
    series.total_issues ??
    0;

  return (
    <div className="space-y-10">
      {/* Ancestor trail — `Library › <library name>`. The series-name H1
          below is the leaf, so the trail stays the parents (mirrors the
          admin PageHeader breadcrumbs). The library segment scopes the
          home grid to this series' library (`?library=<id>`). */}
      <Breadcrumbs
        items={[
          { label: "Library", href: "/" },
          ...(library
            ? [{ label: library.name, href: `/?library=${library.id}` }]
            : []),
        ]}
      />

      <header className="grid grid-cols-1 gap-6 sm:gap-8 lg:grid-cols-[18rem_1fr]">
        {/* v0.5.10 mobile hero reshape — mirrors the issue page so
            the two surfaces feel consistent on phones. Cover grows
            to ~82% viewport width on mobile; Read + Actions collapse
            to a single 48px-tall row with Actions as an icon-only
            kebab. sm+ keeps the prior stacked sidebar layout. */}
        <div className="flex flex-col gap-3 sm:gap-4">
          <div className="mx-auto w-4/5 max-w-full sm:w-56 lg:mx-0 lg:w-72">
            <Cover
              src={series.cover_url}
              alt={`Cover of ${series.name}`}
              fallback={series.publisher ?? "—"}
            />
          </div>
          {/* Read CTA + a small gear for actions, always side by side —
              the Read button is the focus, so actions stay a compact icon
              to its right rather than a full-width stacked button. */}
          <div className="mx-auto flex w-full max-w-xs flex-row gap-2 sm:max-w-sm lg:mx-0 lg:max-w-72">
            {nextHref ? (
              <Button asChild size="lg" className="h-12 flex-1 sm:h-10">
                <Link href={nextHref}>{readButtonLabel(nextState)}</Link>
              </Button>
            ) : (
              <p className="border-border text-muted-foreground flex h-12 flex-1 items-center justify-center rounded-md border border-dashed px-4 text-center text-xs sm:h-10">
                No active issues to read.
              </p>
            )}
            <SeriesActions
              series={series}
              libraryId={series.library_id}
              firstIssue={firstIssue}
              readIncognitoHref={readIncognitoHref}
            />
          </div>
        </div>

        {/* Right column — title, inline icon-driven facts row, summary,
            chips. Per the user's request, series-level data (publisher,
            year, reading time, etc.) is surfaced front and center rather
            than buried under a tab. */}
        <div className="min-w-0 space-y-5">
          <div>
            {/* Prominent volume caption above the H1 — mirrors the
                issue page's `Vol. N` badge so the multi-volume signal
                is visible without drilling into the Details tab. Only
                rendered when the series carries a volume number;
                single-volume titles (volume null) just show the H1. */}
            {series.volume != null && (
              <p className="text-muted-foreground mb-1 text-sm font-medium tracking-wide uppercase">
                Volume {series.volume}
              </p>
            )}
            <h1 className="text-3xl font-semibold tracking-tight sm:text-4xl">
              {series.name}
            </h1>
            <SeriesFactRow series={series} readingTime={readingTime} />
            <div className="mt-3 flex flex-wrap items-center gap-2">
              {status && <Badge variant="outline">{status}</Badge>}
              <CollectionBadge series={series} />
              {series.age_rating && (
                <Badge variant="secondary">{series.age_rating}</Badge>
              )}
              <UserRating
                scope="series"
                seriesSlug={series.slug}
                initial={series.user_rating ?? null}
                label="Series rating"
                variant="inline"
              />
              <ProviderBadgesRow scope="series" seriesSlug={series.slug} />
            </div>
          </div>
          <Description text={series.summary} />

          <div className="grid gap-x-6 gap-y-4 sm:grid-cols-2">
            <FactBlock label="Writers">
              {series.writers && series.writers.length > 0 ? (
                <ChipList
                  items={series.writers}
                  filterField="writer"
                  creatorSlugs={series.creator_slugs}
                />
              ) : (
                <p className="text-muted-foreground text-sm">—</p>
              )}
            </FactBlock>
            <FactBlock label="Publication">
              <p className="text-sm">{status ?? "—"}</p>
            </FactBlock>
            {hasGenres && (
              <FactBlock label="Genres">
                <ChipList items={series.genres} filterField="genres" />
              </FactBlock>
            )}
            {hasTags && (
              <FactBlock label="Tags">
                <ChipList items={series.tags} filterField="tags" />
              </FactBlock>
            )}
          </div>
        </div>
      </header>

      <section className="grid grid-cols-2 gap-3 sm:grid-cols-4">
        <Stat label="Status" value={status} />
        <Stat label="Released" value={releasedLabel} />
        <ReadingLoadStat
          totalPages={series.total_page_count ?? null}
          finishedPages={series.progress_summary?.finished_pages ?? 0}
        />
        <ReadProgressStat read={finishedCount} total={totalCount} />
      </section>

      <Tabs defaultValue="credits">
        <TabsList>
          <TabsTrigger value="credits">Credits</TabsTrigger>
          <TabsTrigger value="cast">Cast &amp; Setting</TabsTrigger>
          <TabsTrigger value="details">Details</TabsTrigger>
          <TabsTrigger value="collection">Collection</TabsTrigger>
          <TabsTrigger value="activity">Activity</TabsTrigger>
        </TabsList>
        {/* Keep the common, lightweight metadata tabs in one force-mounted
            grid cell so the issue list below does not jump between Credits
            and Cast. Details / Collection / Activity render on demand because
            they can be substantially taller and should not reserve blank
            space when inactive. */}
        <StableTabsPanelStack>
          <StableTabsPanel value="credits">
            <div className="divide-border/60 divide-y">
              <ChipList
                orientation="horizontal"
                className="py-3 first:pt-0 last:pb-0"
                label="Writers"
                items={series.writers}
                filterField="writer"
                creatorSlugs={series.creator_slugs}
              />
              <ChipList
                orientation="horizontal"
                className="py-3 first:pt-0 last:pb-0"
                label="Pencillers"
                items={series.pencillers}
                filterField="penciller"
                creatorSlugs={series.creator_slugs}
              />
              <ChipList
                orientation="horizontal"
                className="py-3 first:pt-0 last:pb-0"
                label="Inkers"
                items={series.inkers}
                filterField="inker"
                creatorSlugs={series.creator_slugs}
              />
              <ChipList
                orientation="horizontal"
                className="py-3 first:pt-0 last:pb-0"
                label="Colorists"
                items={series.colorists}
                filterField="colorist"
                creatorSlugs={series.creator_slugs}
              />
              <ChipList
                orientation="horizontal"
                className="py-3 first:pt-0 last:pb-0"
                label="Letterers"
                items={series.letterers}
                filterField="letterer"
                creatorSlugs={series.creator_slugs}
              />
              <ChipList
                orientation="horizontal"
                className="py-3 first:pt-0 last:pb-0"
                label="Cover artists"
                items={series.cover_artists}
                filterField="cover_artist"
                creatorSlugs={series.creator_slugs}
              />
            </div>
            {!hasAny(
              series.writers,
              series.pencillers,
              series.inkers,
              series.colorists,
              series.letterers,
              series.cover_artists,
            ) && (
              <p className="text-muted-foreground text-sm">
                No creator metadata across this series&rsquo;s issues.
              </p>
            )}
          </StableTabsPanel>
          <StableTabsPanel value="cast">
            <div className="divide-border/60 divide-y">
              <ChipList
                orientation="horizontal"
                className="py-3 first:pt-0 last:pb-0"
                label="Characters"
                items={series.characters}
                filterField="characters"
              />
              <ChipList
                orientation="horizontal"
                className="py-3 first:pt-0 last:pb-0"
                label="Teams"
                items={series.teams}
                filterField="teams"
              />
              <ChipList
                orientation="horizontal"
                className="py-3 first:pt-0 last:pb-0"
                label="Locations"
                items={series.locations}
                filterField="locations"
              />
            </div>
            {!hasAny(series.characters, series.teams, series.locations) && (
              <p className="text-muted-foreground text-sm">
                No cast or setting metadata across this series.
              </p>
            )}
          </StableTabsPanel>
          <StackedTabsPanel value="details" className="space-y-6">
            <DetailSummaryGrid>
              <DetailSummaryItem
                label="Issues"
                value={formatIssueTotal(series)}
                hint={formatCollectionHint(series)}
                icon={<BookOpen className="h-4 w-4" />}
              />
              <DetailSummaryItem
                label="Published"
                value={releasedLabel}
                hint={status}
                icon={<Calendar className="h-4 w-4" />}
              />
              <DetailSummaryItem
                label="Length"
                value={formatPageCount(series.total_page_count)}
                hint={readingTime ? `≈ ${readingTime}` : null}
                icon={<FileStack className="h-4 w-4" />}
              />
              <DetailSummaryItem
                label="Updated"
                value={formatRelativeDate(
                  series.last_issue_updated_at ?? series.updated_at,
                )}
                hint="Issue metadata"
                icon={<Clock className="h-4 w-4" />}
              />
            </DetailSummaryGrid>

            <div className="grid gap-4 xl:grid-cols-2">
              <DetailSection
                title="Publication"
                description="Series identity, release range, and reading defaults."
              >
                <MetadataGrid
                  columns={2}
                  items={[
                    { label: "Series name", value: series.name },
                    { label: "Publisher", value: series.publisher },
                    { label: "Volume", value: series.volume },
                    { label: "Release years", value: releasedLabel },
                    {
                      label: "Publication status",
                      value: status ? (
                        <Badge variant="outline">{status}</Badge>
                      ) : null,
                    },
                    {
                      label: "Reading direction",
                      value:
                        series.reading_direction === "rtl"
                          ? "Right-to-left"
                          : series.reading_direction === "ltr"
                            ? "Left-to-right"
                            : "Auto",
                    },
                    {
                      label: "Age rating",
                      value: series.age_rating ? (
                        <Badge variant="secondary">{series.age_rating}</Badge>
                      ) : null,
                    },
                    {
                      label: "Language",
                      value: series.language_code ? (
                        <Badge variant="secondary">
                          {series.language_code.toUpperCase()}
                        </Badge>
                      ) : null,
                    },
                  ]}
                />
              </DetailSection>

              <DetailSection
                title="Library & reading"
                description="Local collection size, estimated reading load, and scanner freshness."
              >
                <MetadataGrid
                  columns={2}
                  items={[
                    {
                      label: "Owned issues",
                      value: series.issue_count,
                    },
                    {
                      label: "Expected issues",
                      value: series.total_issues,
                    },
                    {
                      label: "Collection",
                      value: collectionStatus(series) ? (
                        <CollectionBadge series={series} />
                      ) : null,
                    },
                    {
                      label: "Total pages",
                      value: formatPageCount(series.total_page_count),
                    },
                    {
                      label: "Reading time",
                      value: readingTime ? `≈ ${readingTime}` : null,
                    },
                    {
                      label: "Last issue added",
                      value: formatRelativeDate(
                        series.last_issue_added_at ?? series.updated_at,
                      ),
                    },
                    {
                      label: "Last issue updated",
                      value: formatRelativeDate(
                        series.last_issue_updated_at ?? series.updated_at,
                      ),
                    },
                    {
                      label: "Series added",
                      value: formatRelativeDate(series.created_at),
                    },
                  ]}
                />
              </DetailSection>
            </div>

            <DetailSection
              title="Classification"
              description="Searchable genre and tag metadata rolled up from this series."
            >
              <div className="divide-border/60 divide-y">
                <ChipList
                  orientation="horizontal"
                  className="py-3 first:pt-0 last:pb-0"
                  label="Genres"
                  items={series.genres}
                  filterField="genres"
                />
                <ChipList
                  orientation="horizontal"
                  className="py-3 first:pt-0 last:pb-0"
                  label="Tags"
                  items={series.tags}
                  filterField="tags"
                />
              </div>
              {!hasAny(series.genres, series.tags) && (
                <p className="text-muted-foreground pt-1 text-sm">
                  No genres or tags in this series&rsquo;s metadata.
                </p>
              )}
            </DetailSection>

            <DetailSection
              title="External IDs"
              description="Provider identifiers linked to this series."
            >
              <ExternalIdsCard
                entityType="series"
                seriesSlug={series.slug}
                chrome="bare"
              />
            </DetailSection>
          </StackedTabsPanel>
          <StackedTabsPanel value="collection">
            <CollectionTab seriesSlug={series.slug} />
          </StackedTabsPanel>
          <StackedTabsPanel value="activity">
            <SeriesActivityTab
              seriesId={series.id}
              seriesSlug={series.slug}
              issues={firstIssuePage.items}
              totalIssueCount={
                series.progress_summary?.total ??
                series.issue_count ??
                series.total_issues ??
                null
              }
            />
          </StackedTabsPanel>
        </StableTabsPanelStack>
      </Tabs>

      <IssuesPanel
        seriesSlug={series.slug}
        issueCount={series.issue_count ?? series.total_issues ?? null}
        initialQuery={initialQuery ?? ""}
      />

      <SeriesSourcesFooter seriesSlug={series.slug} />
    </div>
  );
}

/**
 * Inline icon-driven row of series-level facts that aren't ComicInfo issue
 * metadata: publisher, total page count, reading time, publication year,
 * language. Keeps the per-fact cells compact so the row fits on one line at
 * lg+ and wraps gracefully below.
 */
function SeriesFactRow({
  series,
  readingTime,
}: {
  series: SeriesView;
  readingTime: string | null;
}) {
  const facts: { icon: React.ReactNode; label: string }[] = [];
  if (series.publisher) {
    facts.push({
      icon: <Building2 className="h-4 w-4" />,
      label: series.publisher,
    });
  }
  if (series.total_page_count) {
    facts.push({
      icon: <FileStack className="h-4 w-4" />,
      label: `${formatCompactPages(series.total_page_count)} pages`,
    });
  }
  if (readingTime) {
    facts.push({
      icon: <Clock className="h-4 w-4" />,
      label: `~${readingTime}`,
    });
  }
  if (series.year) {
    facts.push({
      icon: <Calendar className="h-4 w-4" />,
      label: String(series.year),
    });
  }
  if (series.language_code) {
    facts.push({
      icon: <Languages className="h-4 w-4" />,
      label: series.language_code.toUpperCase(),
    });
  }
  if (facts.length === 0) return null;
  return (
    <div className="text-muted-foreground mt-2 flex flex-wrap items-center gap-x-3 gap-y-1 text-xs sm:gap-x-4 sm:gap-y-2 sm:text-sm">
      {facts.map((f, i) => (
        <span key={i} className="inline-flex items-center gap-1.5">
          {/* Icons drop on mobile so the row stays compact and on a
              single visual line. Matches the issue page's `IssueFactRow`. */}
          <span className="text-muted-foreground/80 hidden sm:inline">
            {f.icon}
          </span>
          <span>{f.label}</span>
        </span>
      ))}
    </div>
  );
}

function FactBlock({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}) {
  return (
    <div className="space-y-1.5">
      {/* A field label, not a document section — keep it out of the heading
          outline so the page doesn't jump h1 → h3 (audit E9). */}
      <p className="text-muted-foreground text-xs font-semibold tracking-wider uppercase">
        {label}
      </p>
      {children}
    </div>
  );
}

function readStateFromResume(state: string): ReadState {
  if (state === "in_progress" || state === "finished") return state;
  return "unread";
}

function resumeReaderHref(resume: SeriesResumeView): string | null {
  if (!resume.issue_slug) return null;
  const base = readerUrl(resume.series_slug, resume.issue_slug);
  return resume.page > 0 ? `${base}?page=${resume.page}` : base;
}

/**
 * Render a year span as "2012", "2012–2018", or `null` when no year is
 * known. The em-dash is `–` (U+2013), not a hyphen, since this is a date
 * range; both the start and end inputs are nullable so callers can drop
 * a single value through unchanged.
 */
function formatYearRange(
  start: number | null,
  end: number | null,
): string | null {
  if (start == null && end == null) return null;
  const lo = start ?? end!;
  const hi = end ?? start!;
  if (lo === hi) return String(lo);
  return `${lo}–${hi}`;
}

function formatIssueTotal(series: SeriesView): string | null {
  const owned = series.issue_count ?? null;
  const expected = series.total_issues ?? null;
  if (owned == null && expected == null) return null;
  if (owned != null && expected != null && expected > 0) {
    return `${owned} / ${expected}`;
  }
  return String(owned ?? expected);
}

function formatCollectionHint(series: SeriesView): string | null {
  const state = collectionStatus(series);
  if (!state) return null;
  return state === "complete" ? "Complete collection" : "Incomplete collection";
}

/**
 * Read-progress card for the stats grid. Replaces the duplicative
 * "Total pages" / "Reading time" / "Last updated" tiles with a meaningful
 * per-user metric: how many of this series's active issues the user has
 * finished. Empty bar at 0/N, full + accent label at N/N. Total pages and
 * reading time still live in the Details tab for curators who want them.
 */
function ReadProgressStat({ read, total }: { read: number; total: number }) {
  const pct =
    total > 0
      ? Math.max(0, Math.min(100, Math.round((read / total) * 100)))
      : 0;
  const complete = total > 0 && read === total;
  return (
    <div className="border-border bg-card flex flex-col gap-2 rounded-md border px-4 py-3">
      <div className="flex items-baseline justify-between">
        <span className="text-muted-foreground text-xs font-medium tracking-wider uppercase">
          Read progress
        </span>
        <span className="text-muted-foreground text-xs">
          {total > 0 ? `${pct}%` : "—"}
        </span>
      </div>
      <span className="text-lg leading-tight font-semibold">
        {total > 0 ? `${read} / ${total}` : "—"}
      </span>
      <Progress value={pct} aria-label={`Read ${read} of ${total} issues`} />
      <span
        className={
          complete
            ? "text-primary text-xs font-medium"
            : "text-muted-foreground text-xs"
        }
      >
        {total === 0
          ? "No active issues"
          : complete
            ? "Series complete"
            : `${total - read} left`}
      </span>
    </div>
  );
}

/**
 * Reading-load card for the stats grid. Surfaces estimated time + page
 * count remaining to finish the series. Once the user has finished every
 * active issue, the card flips to a "re-read" estimate over the full page
 * count so it stays useful for completionists deciding whether to start
 * over. Mirrors the `<Stat>` shape so it sits flush next to the other
 * tiles. `total_page_count` is the server-aggregated sum across active
 * issues; we subtract `progress_summary.finished_pages` to get the
 * remainder without paginating the issue list client-side.
 */
function ReadingLoadStat({
  totalPages,
  finishedPages,
}: {
  totalPages: number | null;
  finishedPages: number;
}) {
  if (totalPages == null || totalPages <= 0) {
    return (
      <div className="border-border bg-card flex flex-col gap-1 rounded-md border px-4 py-3">
        <span className="text-muted-foreground text-xs font-medium tracking-wider uppercase">
          Reading load remaining
        </span>
        <span className="text-lg leading-tight font-semibold">—</span>
      </div>
    );
  }
  const remainingPages = Math.max(0, totalPages - finishedPages);
  const isReread = remainingPages === 0;
  // When the user has finished the whole series, fall back to the full
  // page count — the card now estimates a re-read instead of a finish.
  const pagesShown = isReread ? totalPages : remainingPages;
  const time = formatReadingTimeCompact(pagesShown);
  return (
    <div className="border-border bg-card flex flex-col gap-1 rounded-md border px-4 py-3">
      <div className="flex items-baseline justify-between">
        <span className="text-muted-foreground text-xs font-medium tracking-wider uppercase">
          Reading load remaining
        </span>
        {isReread ? (
          <span className="text-primary text-xs font-medium">re-read</span>
        ) : null}
      </div>
      <span className="text-lg leading-tight font-semibold tabular-nums">
        {time ?? "—"}
      </span>
      <span className="text-muted-foreground text-xs">
        {formatCompactPages(pagesShown)} pages
      </span>
    </div>
  );
}

function hasAny(...lists: (string[] | undefined)[]): boolean {
  return lists.some((l) => Array.isArray(l) && l.length > 0);
}

/** Renders a Complete / Incomplete badge derived from `total_issues`
 *  vs. `issue_count`. Returns nothing when the helper has no signal
 *  (no `total_issues` known) so the row stays clean for series the
 *  scanner hasn't flagged yet. */
function CollectionBadge({ series }: { series: SeriesView }) {
  const state = collectionStatus(series);
  if (!state) return null;
  const have = series.issue_count ?? 0;
  const total = series.total_issues ?? 0;
  const tooltip =
    state === "complete"
      ? `Complete: ${have} of ${total} issues`
      : `${have} of ${total} issues`;
  return state === "complete" ? (
    <Badge
      variant="secondary"
      className={statusTone("success")}
      title={tooltip}
    >
      Complete
    </Badge>
  ) : (
    <Badge
      variant="secondary"
      className={statusTone("warning")}
      title={tooltip}
    >
      Incomplete
    </Badge>
  );
}
