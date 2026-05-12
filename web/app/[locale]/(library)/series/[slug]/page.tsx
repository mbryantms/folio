import {
  ChevronRight,
  Building2,
  FileStack,
  Clock,
  Calendar,
  Languages,
} from "lucide-react";
import Link from "next/link";
import { notFound, redirect } from "next/navigation";

import { Cover } from "@/components/Cover";
import { ChipList } from "@/components/library/ChipList";
import { Description } from "@/components/library/Description";
import { MetadataGrid } from "@/components/library/MetadataGrid";
import { Stat } from "@/components/library/Stat";
import { UserRating } from "@/components/library/UserRating";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Progress } from "@/components/ui/progress";
import { SeriesActivityTab } from "@/components/activity/SeriesActivityTab";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { apiGet, ApiError } from "@/lib/api/fetch";
import type {
  IssueListView,
  IssueSummaryView,
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
import {
  type ProgressLike,
  type ReadState,
  indexProgress,
  pickNextIssue,
  readButtonLabel,
} from "@/lib/reading-state";

import { readerUrl } from "@/lib/urls";

import { IssuesPanel } from "./IssuesPanel";
import { SeriesActions } from "./SeriesActions";

type ProgressDelta = { records: ProgressLike[] };

export default async function SeriesPage({
  params,
}: {
  params: Promise<{ slug: string }>;
}) {
  const { slug } = await params;

  let series: SeriesView;
  let firstPage: IssueListView;
  try {
    series = await apiGet<SeriesView>(`/series/${slug}`);
    // First page used purely for resume detection. Client component
    // re-fetches with the chosen sort/search.
    firstPage = await apiGet<IssueListView>(`/series/${slug}/issues?limit=100`);
  } catch (e) {
    if (e instanceof ApiError) {
      if (e.status === 401) redirect(`/sign-in`);
      if (e.status === 404) notFound();
    }
    throw e;
  }

  const next = await pickNextWithProgress(firstPage.items);
  // The first active issue is the "Read from beginning" target — independent
  // of the resume target so users can always restart from #1 even when
  // they're mid-way through a later issue.
  const firstIssue = firstPage.items.find((i) => i.state === "active") ?? null;

  const status = formatPublicationStatus(series.status);
  const readingTime = formatReadingTime(series.total_page_count ?? 0);
  const releasedLabel = formatYearRange(
    series.earliest_year ?? series.year ?? null,
    series.latest_year ?? null,
  );

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
      <nav
        aria-label="Breadcrumb"
        className="text-muted-foreground flex items-center gap-1.5 text-xs"
      >
        <Link
          href={`/`}
          className="hover:text-foreground underline-offset-2 hover:underline"
        >
          Library
        </Link>
        <ChevronRight className="h-3 w-3" />
        <span className="text-foreground/80">{series.name}</span>
      </nav>

      <header className="grid grid-cols-1 gap-8 lg:grid-cols-[18rem_1fr]">
        {/* Cover column — bigger than before, with the primary CTA stacked
            directly underneath so it falls within natural eye-flow from
            cover → action. */}
        <div className="flex flex-col gap-4">
          <div className="mx-auto w-72 max-w-full lg:mx-0">
            <Cover
              src={series.cover_url}
              alt={`Cover of ${series.name}`}
              fallback={series.publisher ?? "—"}
            />
          </div>
          <div className="flex flex-col gap-2 lg:max-w-72">
            {next.target ? (
              <Button asChild size="lg" className="w-full">
                <Link href={readerUrl(next.target)}>
                  {readButtonLabel(next.state)}
                  {next.target.number ? ` · #${next.target.number}` : ""}
                </Link>
              </Button>
            ) : (
              <p className="border-border text-muted-foreground rounded-md border border-dashed px-4 py-2 text-center text-xs">
                No active issues to read.
              </p>
            )}
            <SeriesActions
              series={series}
              libraryId={series.library_id}
              firstIssueId={firstIssue?.id ?? null}
            />
          </div>
        </div>

        {/* Right column — title, inline icon-driven facts row, summary,
            chips. Per the user's request, series-level data (publisher,
            year, reading time, etc.) is surfaced front and center rather
            than buried under a tab. */}
        <div className="min-w-0 space-y-5">
          <div>
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
            </div>
          </div>
          <Description text={series.summary} />

          <div className="grid gap-x-6 gap-y-4 sm:grid-cols-2">
            <FactBlock label="Writers">
              {series.writers && series.writers.length > 0 ? (
                <ChipList items={series.writers} filterField="writer" />
              ) : (
                <p className="text-muted-foreground text-sm">—</p>
              )}
            </FactBlock>
            <FactBlock label="Publication">
              <p className="text-sm">{status ?? "—"}</p>
            </FactBlock>
            <FactBlock label="Genres">
              {series.genres && series.genres.length > 0 ? (
                <ChipList items={series.genres} filterField="genres" />
              ) : (
                <p className="text-muted-foreground text-sm">—</p>
              )}
            </FactBlock>
            <FactBlock label="Tags">
              {series.tags && series.tags.length > 0 ? (
                <ChipList items={series.tags} filterField="tags" />
              ) : (
                <p className="text-muted-foreground text-sm">—</p>
              )}
            </FactBlock>
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
          <TabsTrigger value="genres">Genres &amp; Tags</TabsTrigger>
          <TabsTrigger value="cast">Cast &amp; Setting</TabsTrigger>
          <TabsTrigger value="details">Details</TabsTrigger>
          <TabsTrigger value="activity">Activity</TabsTrigger>
        </TabsList>
        <TabsContent value="credits" className="pt-6">
          <div className="grid gap-6 sm:grid-cols-2">
            <ChipList
              label="Writers"
              items={series.writers}
              filterField="writer"
            />
            <ChipList
              label="Pencillers"
              items={series.pencillers}
              filterField="penciller"
            />
            <ChipList
              label="Inkers"
              items={series.inkers}
              filterField="inker"
            />
            <ChipList
              label="Colorists"
              items={series.colorists}
              filterField="colorist"
            />
            <ChipList
              label="Letterers"
              items={series.letterers}
              filterField="letterer"
            />
            <ChipList
              label="Cover artists"
              items={series.cover_artists}
              filterField="cover_artist"
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
        </TabsContent>
        <TabsContent value="genres" className="pt-6">
          <div className="grid gap-6 sm:grid-cols-2">
            <ChipList
              label="Genres"
              items={series.genres}
              filterField="genres"
            />
            <ChipList label="Tags" items={series.tags} filterField="tags" />
          </div>
          {!hasAny(series.genres, series.tags) && (
            <p className="text-muted-foreground text-sm">
              No genres or tags in this series&rsquo;s metadata.
            </p>
          )}
        </TabsContent>
        <TabsContent value="cast" className="pt-6">
          <div className="grid gap-6 sm:grid-cols-2">
            <ChipList
              label="Characters"
              items={series.characters}
              filterField="characters"
            />
            <ChipList label="Teams" items={series.teams} filterField="teams" />
            <ChipList
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
        </TabsContent>
        <TabsContent value="details" className="pt-6">
          <MetadataGrid
            items={[
              { label: "Series name", value: series.name },
              { label: "Publisher", value: series.publisher },
              { label: "Volume", value: series.volume },
              { label: "Year", value: series.year },
              { label: "Status", value: status },
              { label: "Age rating", value: series.age_rating },
              {
                label: "Language",
                value: series.language_code?.toUpperCase(),
              },
              {
                label: "Issues",
                value: series.issue_count ?? series.total_issues ?? null,
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
                label: "ComicVine ID",
                value:
                  series.comicvine_id != null
                    ? String(series.comicvine_id)
                    : null,
              },
              {
                label: "Metron ID",
                value:
                  series.metron_id != null ? String(series.metron_id) : null,
              },
              { label: "GTIN", value: null },
            ]}
          />
        </TabsContent>
        <TabsContent value="activity" className="pt-6">
          <SeriesActivityTab
            seriesId={series.id}
            seriesSlug={series.slug}
            issues={firstPage.items}
            totalIssueCount={
              series.progress_summary?.total ??
              series.issue_count ??
              series.total_issues ??
              null
            }
          />
        </TabsContent>
      </Tabs>

      <IssuesPanel
        seriesSlug={series.slug}
        issueCount={series.issue_count ?? series.total_issues ?? null}
      />
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
    <div className="text-muted-foreground mt-2 flex flex-wrap items-center gap-x-4 gap-y-2 text-sm">
      {facts.map((f, i) => (
        <span key={i} className="inline-flex items-center gap-1.5">
          <span className="text-muted-foreground/80">{f.icon}</span>
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
      <h3 className="text-muted-foreground text-xs font-semibold tracking-wider uppercase">
        {label}
      </h3>
      {children}
    </div>
  );
}

/**
 * Resolve the dynamic CTA target ("Read" / "Continue reading" / "Read
 * again"). Uses just the first 100 issues for resume detection — that's
 * enough for almost every series, and the *aggregate* read-progress
 * counts now come from `series.progress_summary` (server-computed, not
 * bound by the client-side page cap).
 */
async function pickNextWithProgress(
  issues: IssueSummaryView[],
): Promise<{ target: IssueSummaryView | null; state: ReadState }> {
  const issueIds = new Set(issues.map((i) => i.id));
  let progressByIssueId = new Map<string, ProgressLike>();
  try {
    const delta = await apiGet<ProgressDelta>(`/progress`);
    progressByIssueId = indexProgress(delta.records, issueIds);
  } catch {
    /* no progress yet */
  }
  return pickNextIssue(issues, progressByIssueId);
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
      className="border-emerald-500/40 bg-emerald-500/10 text-emerald-700 dark:text-emerald-400"
      title={tooltip}
    >
      Complete
    </Badge>
  ) : (
    <Badge
      variant="secondary"
      className="border-amber-500/40 bg-amber-500/10 text-amber-700 dark:text-amber-400"
      title={tooltip}
    >
      Incomplete
    </Badge>
  );
}
