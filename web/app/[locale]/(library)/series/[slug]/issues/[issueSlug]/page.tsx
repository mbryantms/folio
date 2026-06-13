import {
  ArrowLeft,
  ArrowRight,
  Building2,
  Calendar,
  ChevronLeft,
  ChevronRight,
  Clock,
  FileStack,
  HardDrive,
  Hash,
  History,
  Languages,
} from "lucide-react";
import Link from "next/link";
import { notFound, redirect } from "next/navigation";

import { issueUrl, readerUrl, seriesUrl } from "@/lib/urls";
import { IssueActivityTab } from "@/components/activity/IssueActivityTab";
import { Cover } from "@/components/Cover";
import { ChipList } from "@/components/library/ChipList";
import {
  DetailSection,
  DetailSummaryGrid,
  DetailSummaryItem,
} from "@/components/library/DetailSection";
import { Description } from "@/components/library/Description";
import { IssueHealthBadge } from "@/components/library/IssueHealthBadge";
import { CoverGallery } from "@/components/library/CoverGallery";
import { HorizontalScrollRail } from "@/components/library/HorizontalScrollRail";
import { ProviderBadgesRow } from "@/components/library/ProviderBadgesRow";
import {
  StableTabsPanel,
  StableTabsPanelStack,
  StackedTabsPanel,
} from "@/components/library/StableTabsPanelStack";

import { IssueSourcesFooter } from "./IssueMetadataPanel";
import { MetadataGrid } from "@/components/library/MetadataGrid";
import { Stat } from "@/components/library/Stat";
import { UserRating } from "@/components/library/UserRating";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { apiGet, ApiError } from "@/lib/api/fetch";
import type {
  ExternalIdRow,
  ExternalIdsListResp,
  IssueDetailView,
  IssueSummaryView,
  NextInSeriesView,
  PrevInSeriesView,
  ReadingStatsView,
  SeriesView,
} from "@/lib/api/types";
import {
  formatPageCount,
  formatPublicationDate,
  formatPublicationStatus,
  formatReadingTime,
  formatRelativeDate,
} from "@/lib/format";
import { cn } from "@/lib/utils";
import { statusTone } from "@/lib/ui/status-tone";
import {
  type ProgressLike,
  type ReadState,
  readButtonLabel,
  readStateFor,
} from "@/lib/reading-state";

import { InlineNotesEditor } from "./InlineNotesEditor";
import { IssueActions } from "./IssueActions";
import { IssueMetadataTab } from "./IssueMetadataTab";

export default async function IssuePage({
  params,
  searchParams,
}: {
  params: Promise<{ slug: string; issueSlug: string }>;
  searchParams: Promise<{ cbl?: string }>;
}) {
  const { slug: seriesSlug, issueSlug } = await params;
  // Propagate the CBL reading-context onto the primary "Read" CTA so
  // a user arriving here via a CBL link keeps their next-up resolver
  // tied to the list when they hit Read.
  const { cbl } = await searchParams;
  const cblSavedViewId = typeof cbl === "string" && cbl.length > 0 ? cbl : null;
  let issue: IssueDetailView;
  try {
    issue = await apiGet<IssueDetailView>(
      `/series/${seriesSlug}/issues/${issueSlug}`,
    );
  } catch (e) {
    if (e instanceof ApiError) {
      if (e.status === 401) redirect(`/sign-in`);
      if (e.status === 404) notFound();
    }
    throw e;
  }
  // Six best-effort fetches, all independent once `issue` is in hand —
  // run them concurrently. They used to await in series, stacking six
  // round-trips of pure latency onto TTFB (≈500ms at 80ms RTT for
  // remote/PWA readers). Each fails soft to its empty value:
  //  - series: breadcrumb + status badge
  //  - progress: dynamic read-CTA label ("no record yet" on 4xx)
  //  - reading stats: "Last read" line + activity strip + Activity tab
  //  - next 5 / prev 1 issues: the filmstrip rail
  //  - external ids: provider rows on the Metadata tab
  const [
    series,
    issueProgress,
    activityStats,
    nextIssues,
    prevIssue,
    issueExternalIds,
  ] = await Promise.all([
    apiGet<SeriesView>(`/series/${seriesSlug}`).catch(() => null),
    apiGet<{ records: ProgressLike[] }>(`/progress`)
      .then(
        (delta) => delta.records.find((r) => r.issue_id === issue.id) ?? null,
      )
      .catch(() => null),
    apiGet<ReadingStatsView>(
      `/me/reading-stats?range=all&issue_id=${encodeURIComponent(issue.id)}`,
    ).catch(() => null),
    apiGet<NextInSeriesView>(
      `/series/${seriesSlug}/issues/${issueSlug}/next?limit=5`,
    )
      .then((next) => next.items)
      .catch(() => [] as IssueSummaryView[]),
    apiGet<PrevInSeriesView>(`/series/${seriesSlug}/issues/${issueSlug}/prev`)
      .then((prev) => prev.item ?? null)
      .catch(() => null),
    apiGet<ExternalIdsListResp>(
      `/series/${encodeURIComponent(seriesSlug)}/issues/${encodeURIComponent(issueSlug)}/external-ids`,
    )
      .then((externalIds) => externalIds.rows)
      .catch(() => [] as ExternalIdRow[]),
  ]);
  const hasActivity = (activityStats?.totals.sessions ?? 0) > 0;

  const readState: ReadState = readStateFor(issue, issueProgress);
  const readLabel = readButtonLabel(readState);

  const heading =
    issue.title ??
    (series && issue.number
      ? `${series.name} #${issue.number}`
      : (series?.name ?? "Issue"));
  const publicationDate = formatPublicationDate(
    issue.year,
    issue.month,
    issue.day,
  );
  const seriesStatus = formatPublicationStatus(series?.status);
  const readingTime = formatReadingTime(issue.page_count);
  const readingDirectionRaw =
    issue.series_reading_direction ?? issue.library_default_reading_direction;
  const readingDirection =
    readingDirectionRaw === "rtl"
      ? "Right-to-left"
      : readingDirectionRaw === "ltr"
        ? "Left-to-right"
        : (readingDirectionRaw ?? null);
  const detailsWebUrl = isProviderWebUrlDuplicate(
    issue.web_url,
    issueExternalIds,
  )
    ? null
    : issue.web_url;

  return (
    <div className="space-y-10">
      <nav aria-label="Breadcrumb" className="text-muted-foreground text-xs">
        {/* Mobile: a back chevron + parent series name only. The H1
            already announces the issue title, so the breadcrumb's
            trailing "All Hope Lies in Doom" was just visual noise on
            a phone-width screen. Hides on sm+ where the full
            breadcrumb fits comfortably. */}
        <div className="flex items-center gap-1.5 sm:hidden">
          <Link
            href={series ? seriesUrl(series) : `/`}
            className="hover:text-foreground inline-flex items-center gap-1 underline-offset-2 hover:underline"
            aria-label={series ? `Back to ${series.name}` : "Back to library"}
          >
            <ChevronLeft className="h-4 w-4" aria-hidden="true" />
            <span>{series?.name ?? "Library"}</span>
          </Link>
        </div>
        {/* sm+ : full breadcrumb (unchanged from pre-v0.5.10). */}
        <div className="hidden flex-wrap items-center gap-1.5 sm:flex">
          <Link
            href={`/`}
            className="hover:text-foreground underline-offset-2 hover:underline"
          >
            Library
          </Link>
          {series && (
            <>
              <ChevronRight className="h-3 w-3" />
              <Link
                href={seriesUrl(series)}
                className="hover:text-foreground underline-offset-2 hover:underline"
              >
                {series.name}
              </Link>
            </>
          )}
          <ChevronRight className="h-3 w-3" />
          <span className="text-foreground/80">{heading}</span>
        </div>
      </nav>

      <header className="grid grid-cols-1 gap-6 sm:gap-8 lg:grid-cols-[18rem_1fr]">
        {/* v0.5.10 mobile hero reshape: cover grows to ~82% viewport
            width on phones so it actually reads as the page's
            primary visual; Read + Actions collapse to a single row
            with the menu as a 48 × 48 kebab. sm+ keeps the prior
            sidebar layout (cover 14rem/18rem column, buttons stacked
            below). */}
        <div className="flex flex-col gap-3 sm:gap-4">
          <div className="mx-auto w-4/5 max-w-full sm:w-56 lg:mx-0 lg:w-72">
            <Cover
              src={
                issue.state === "active"
                  ? `/issues/${issue.id}/pages/0/thumb`
                  : null
              }
              alt={heading}
              fallback={issue.state === "active" ? "Cover" : issue.state}
            />
          </div>
          <div className="mx-auto flex w-full max-w-xs flex-row gap-2 sm:max-w-sm sm:flex-col lg:mx-0 lg:max-w-72">
            {issue.state === "active" ? (
              // h-12 on mobile so the button matches the Actions
              // kebab's 48 × 48 footprint exactly; sm+ falls back to
              // h-10 (Actions menu trigger uses sm:h-10 too, so the
              // stacked CTAs stay flush in the sidebar).
              //
              // `sm:flex-none` cancels mobile's `flex-1` once the
              // container flips to `sm:flex-col` — otherwise
              // flex-grow stretches the button vertically along the
              // column's main axis and visually overrides sm:h-10.
              <Button
                asChild
                size="lg"
                className="h-12 flex-1 sm:h-10 sm:w-full sm:flex-none"
              >
                <Link href={readerUrl(issue, { cbl: cblSavedViewId })}>
                  {readLabel}
                </Link>
              </Button>
            ) : (
              <p className="border-border text-muted-foreground flex h-12 flex-1 items-center justify-center rounded-md border border-dashed px-4 text-center text-xs sm:h-auto sm:flex-none sm:py-2">
                Cannot read — issue state: {issue.state}
              </p>
            )}
            <IssueActions
              issue={issue}
              series={series}
              readState={readState}
              cblSavedViewId={cblSavedViewId}
            />
          </div>
        </div>

        <div className="min-w-0 space-y-5">
          <div>
            <div className="text-muted-foreground flex flex-wrap items-center gap-x-3 gap-y-1 text-sm">
              {series && (
                <Link
                  href={seriesUrl(series)}
                  className="text-foreground font-medium underline-offset-4 hover:underline"
                >
                  {series.name}
                </Link>
              )}
              {issue.number && (
                <span
                  aria-label={`Issue number ${issue.number}`}
                  className="border-primary/50 bg-primary/10 text-primary inline-flex items-center rounded-md border px-2 py-0.5 font-mono text-sm font-semibold tabular-nums"
                >
                  #{issue.number}
                </span>
              )}
              {/* Effective volume: issue's per-issue override (rare —
                  set when ComicInfo's `<Volume>` tag explicitly
                  differs from the parent run), else the parent
                  series's volume. Inherits so a reader on Fantastic
                  Four V6 #4 sees "Vol. 6" without having to navigate
                  back to the series page. */}
              {(issue.volume ?? series?.volume ?? null) != null && (
                <span>Vol. {issue.volume ?? series?.volume}</span>
              )}
            </div>
            <h1 className="mt-2 text-3xl font-semibold tracking-tight sm:text-4xl">
              {heading}
            </h1>
            <IssueFactRow
              issue={issue}
              series={series}
              publicationDate={publicationDate}
              readingTime={readingTime}
              lastReadAt={activityStats?.last_read_at ?? null}
              timesRead={activityStats?.totals.sessions ?? 0}
            />
            <div className="mt-3 flex flex-wrap items-center gap-2">
              {seriesStatus && <Badge variant="outline">{seriesStatus}</Badge>}
              {issue.age_rating && (
                <Badge variant="secondary">{issue.age_rating}</Badge>
              )}
              {issue.format && (
                <Badge variant="secondary">{issue.format}</Badge>
              )}
              {issue.manga && issue.manga !== "No" && (
                <Badge variant="secondary">Manga</Badge>
              )}
              {issue.black_and_white && <Badge variant="secondary">B/W</Badge>}
              {issue.state !== "active" && (
                <Badge variant="destructive">{issue.state}</Badge>
              )}
              <IssueHealthBadge seriesSlug={seriesSlug} issueSlug={issueSlug} />
              {issue.metadata_completeness &&
                issue.metadata_completeness.tier !== "complete" && (
                  <Badge
                    variant="outline"
                    className={
                      issue.metadata_completeness.tier === "needs_metadata"
                        ? statusTone("warning")
                        : undefined
                    }
                    title={
                      issue.metadata_completeness.missing_core.length > 0
                        ? `Missing: ${issue.metadata_completeness.missing_core.join(", ")}`
                        : undefined
                    }
                  >
                    {issue.metadata_completeness.tier === "needs_metadata"
                      ? "Needs metadata"
                      : "Partial metadata"}
                  </Badge>
                )}
              <UserRating
                scope="issue"
                seriesSlug={issue.series_slug}
                issueSlug={issue.slug}
                initial={issue.user_rating ?? null}
                label="Your rating"
                variant="inline"
              />
              <ProviderBadgesRow
                scope="issue"
                seriesSlug={issue.series_slug}
                issueSlug={issue.slug}
              />
            </div>
            {/* "Sidecar metadata refreshed N ago" badge moved into the
                Edit sheet header — it's operator-facing context that
                belonged next to the override controls, not on the
                landing surface. */}
          </div>
          {issue.summary && <Description text={issue.summary} />}
        </div>
      </header>

      <section className="grid grid-cols-2 gap-3 sm:grid-cols-4">
        <Stat label="Writer" value={primaryCredit(issue.writer)} />
        <Stat label="Penciller" value={primaryCredit(issue.penciller)} />
        <Stat label="Released" value={publicationDate} />
        <Stat label="Status" value={seriesStatus} />
      </section>

      {(prevIssue || nextIssues.length > 0) && series && (
        <NextInSeries prev={prevIssue} items={nextIssues} series={series} />
      )}

      <Tabs defaultValue="credits">
        <TabsList>
          <TabsTrigger value="credits">Credits</TabsTrigger>
          <TabsTrigger value="cast">Cast &amp; Setting</TabsTrigger>
          <TabsTrigger value="details">Details</TabsTrigger>
          <TabsTrigger value="covers">Covers</TabsTrigger>
          <TabsTrigger value="metadata">Metadata</TabsTrigger>
          <TabsTrigger value="notes">Notes</TabsTrigger>
          {hasActivity && <TabsTrigger value="activity">Activity</TabsTrigger>}
        </TabsList>

        {/* Credits and Cast are lightweight, high-traffic tabs, so they stay
         * force-mounted in one grid cell and reserve a compact baseline height.
         * Details / Metadata / Covers / Activity render on demand; they can be
         * much taller and should not leave their full height behind when the
         * user returns to the common tabs. */}
        <StableTabsPanelStack>
          <StackedTabsPanel value="details" className="space-y-6">
            <DetailSummaryGrid>
              <DetailSummaryItem
                label="Issue"
                value={issue.number ? `#${issue.number}` : null}
                hint={series?.name ?? null}
                icon={<Hash className="h-4 w-4" />}
              />
              <DetailSummaryItem
                label="Published"
                value={publicationDate}
                hint={seriesStatus}
                icon={<Calendar className="h-4 w-4" />}
              />
              <DetailSummaryItem
                label="Length"
                value={formatPageCount(issue.page_count)}
                hint={readingTime ? `≈ ${readingTime}` : null}
                icon={<FileStack className="h-4 w-4" />}
              />
              <DetailSummaryItem
                label="Archive"
                value={formatFileSize(issue.file_size)}
                hint={issue.state === "active" ? null : issue.state}
                icon={<HardDrive className="h-4 w-4" />}
              />
            </DetailSummaryGrid>

            <div className="grid gap-4 xl:grid-cols-2">
              <DetailSection
                title="Publication"
                description="Series identity, release data, and source page."
              >
                <MetadataGrid
                  columns={2}
                  items={[
                    {
                      label: "Series",
                      value: series ? (
                        <Link
                          href={seriesUrl(series)}
                          className="text-primary font-medium hover:underline"
                        >
                          {series.name}
                        </Link>
                      ) : null,
                    },
                    {
                      label: "Issue number",
                      value: issue.number ? (
                        <Badge variant="outline" className="font-mono">
                          #{issue.number}
                        </Badge>
                      ) : null,
                    },
                    {
                      label: "Volume",
                      value: issue.volume ?? series?.volume ?? null,
                    },
                    {
                      label: "Alternate series",
                      value: issue.alternate_series,
                    },
                    { label: "Publication date", value: publicationDate },
                    {
                      label: "Publication status",
                      value: seriesStatus ? (
                        <Badge variant="outline">{seriesStatus}</Badge>
                      ) : null,
                    },
                    { label: "Publisher", value: issue.publisher },
                    { label: "Imprint", value: issue.imprint },
                    { label: "Story arc", value: issue.story_arc },
                    {
                      label: "Story arc number",
                      value: issue.story_arc_number,
                    },
                    {
                      label: "Web page",
                      value: detailsWebUrl ? (
                        <ExternalTextLink url={detailsWebUrl} />
                      ) : null,
                      wide: true,
                    },
                  ]}
                />
              </DetailSection>

              <DetailSection
                title="Reading & format"
                description="Reader behavior, content rating, and source format."
              >
                <MetadataGrid
                  columns={2}
                  items={[
                    {
                      label: "Language",
                      value: issue.language_code ? (
                        <Badge variant="secondary">
                          {issue.language_code.toUpperCase()}
                        </Badge>
                      ) : null,
                    },
                    { label: "Reading direction", value: readingDirection },
                    {
                      label: "Age rating",
                      value: issue.age_rating ? (
                        <Badge variant="secondary">{issue.age_rating}</Badge>
                      ) : null,
                    },
                    {
                      label: "Format",
                      value: issue.format ? (
                        <Badge variant="secondary">{issue.format}</Badge>
                      ) : null,
                    },
                    {
                      label: "Color",
                      value:
                        issue.black_and_white == null
                          ? null
                          : issue.black_and_white
                            ? "Black & white"
                            : "Color",
                    },
                    {
                      label: "Manga",
                      value: issue.manga ? (
                        <Badge variant="outline">{issue.manga}</Badge>
                      ) : null,
                    },
                    {
                      label: "Pages",
                      value: formatPageCount(issue.page_count),
                    },
                    {
                      label: "Estimated reading time",
                      value: readingTime ? `≈ ${readingTime}` : null,
                    },
                    {
                      label: "GTIN",
                      value: issue.gtin ? (
                        <code className="font-mono text-xs">{issue.gtin}</code>
                      ) : null,
                      wide: true,
                    },
                  ]}
                />
              </DetailSection>
            </div>

            <DetailSection
              title="Classification"
              description="Searchable genre and tag metadata used by library filters."
            >
              <div className="divide-border/60 divide-y">
                <ChipList
                  orientation="horizontal"
                  className="py-3 first:pt-0 last:pb-0"
                  label="Genres"
                  items={splitCsv(issue.genre)}
                  filterField="genres"
                />
                <ChipList
                  orientation="horizontal"
                  className="py-3 first:pt-0 last:pb-0"
                  label="Tags"
                  items={splitCsv(issue.tags)}
                  filterField="tags"
                />
              </div>
              {!issue.genre && !issue.tags && (
                <p className="text-muted-foreground pt-1 text-sm">
                  No genres or tags.
                </p>
              )}
            </DetailSection>

            <DetailSection
              title="Library & file"
              description="Local archive details and scanner timestamps."
            >
              <MetadataGrid
                columns={3}
                items={[
                  {
                    label: "Sort order",
                    value:
                      issue.sort_number != null
                        ? issue.sort_number.toString()
                        : null,
                  },
                  {
                    label: "Cover page",
                    value:
                      issue.cover_page_index != null
                        ? `Page ${issue.cover_page_index + 1}`
                        : null,
                  },
                  {
                    label: "Added",
                    value: formatRelativeDate(issue.created_at),
                  },
                  {
                    label: "Updated",
                    value: formatRelativeDate(issue.updated_at),
                  },
                  {
                    label: "File size",
                    value: formatFileSize(issue.file_size),
                  },
                  {
                    label: "File",
                    value: <FilePathValue path={issue.file_path} />,
                    wide: true,
                  },
                ]}
              />
            </DetailSection>

            {issue.additional_links.length > 0 && (
              <DetailSection
                title="External links"
                description="Additional curated links stored with this issue."
              >
                <MetadataGrid
                  columns={2}
                  items={issue.additional_links.map((link, i) => ({
                    label: link.label ?? `Link ${i + 1}`,
                    value: <ExternalTextLink url={link.url} />,
                    wide: true,
                  }))}
                />
              </DetailSection>
            )}
            {/* "Locally edited fields" summary moved into the Edit sheet
                — fields surface a per-row release control alongside their
                input, so the user can both see what's pinned and release
                it without leaving the editor. */}
          </StackedTabsPanel>

          <StableTabsPanel value="credits">
            <div className="divide-border/60 divide-y">
              <ChipList
                orientation="horizontal"
                className="py-3 first:pt-0 last:pb-0"
                label="Writer"
                items={splitCsv(issue.writer)}
                filterField="writer"
                creatorSlugs={issue.creator_slugs}
              />
              <ChipList
                orientation="horizontal"
                className="py-3 first:pt-0 last:pb-0"
                label="Penciller"
                items={splitCsv(issue.penciller)}
                filterField="penciller"
                creatorSlugs={issue.creator_slugs}
              />
              <ChipList
                orientation="horizontal"
                className="py-3 first:pt-0 last:pb-0"
                label="Inker"
                items={splitCsv(issue.inker)}
                filterField="inker"
                creatorSlugs={issue.creator_slugs}
              />
              <ChipList
                orientation="horizontal"
                className="py-3 first:pt-0 last:pb-0"
                label="Colorist"
                items={splitCsv(issue.colorist)}
                filterField="colorist"
                creatorSlugs={issue.creator_slugs}
              />
              <ChipList
                orientation="horizontal"
                className="py-3 first:pt-0 last:pb-0"
                label="Letterer"
                items={splitCsv(issue.letterer)}
                filterField="letterer"
                creatorSlugs={issue.creator_slugs}
              />
              <ChipList
                orientation="horizontal"
                className="py-3 first:pt-0 last:pb-0"
                label="Cover artist"
                items={splitCsv(issue.cover_artist)}
                filterField="cover_artist"
                creatorSlugs={issue.creator_slugs}
              />
            </div>
            {!hasAnyCredit(issue) && (
              <p className="text-muted-foreground text-sm">
                No creator metadata in this issue.
              </p>
            )}
          </StableTabsPanel>

          <StableTabsPanel value="cast">
            <div className="divide-border/60 divide-y">
              <ChipList
                orientation="horizontal"
                className="py-3 first:pt-0 last:pb-0"
                label="Characters"
                items={splitCsv(issue.characters)}
                filterField="characters"
              />
              <ChipList
                orientation="horizontal"
                className="py-3 first:pt-0 last:pb-0"
                label="Teams"
                items={splitCsv(issue.teams)}
                filterField="teams"
              />
              <ChipList
                orientation="horizontal"
                className="py-3 first:pt-0 last:pb-0"
                label="Locations"
                items={splitCsv(issue.locations)}
                filterField="locations"
              />
              {/* Story arc has no series-level library filter (it's an
                  issue-level concept), so the chip stays read-only. */}
              <ChipList
                orientation="horizontal"
                className="py-3 first:pt-0 last:pb-0"
                label="Story arc"
                items={splitCsv(issue.story_arc)}
              />
            </div>
            {!issue.characters &&
              !issue.teams &&
              !issue.locations &&
              !issue.story_arc && (
                <p className="text-muted-foreground text-sm">
                  No cast or setting metadata.
                </p>
              )}
          </StableTabsPanel>

          <StackedTabsPanel value="metadata">
            <IssueMetadataTab seriesSlug={seriesSlug} issueSlug={issue.slug} />
          </StackedTabsPanel>

          <StackedTabsPanel value="notes">
            <InlineNotesEditor
              seriesSlug={seriesSlug}
              issueSlug={issue.slug}
              initial={issue.notes ?? null}
            />
          </StackedTabsPanel>

          <StackedTabsPanel value="covers">
            <CoverGallery issueId={issue.id} chrome="bare" />
          </StackedTabsPanel>
          {hasActivity && (
            <StackedTabsPanel value="activity">
              <IssueActivityTab
                issueId={issue.id}
                pageCount={issue.page_count ?? null}
              />
            </StackedTabsPanel>
          )}
        </StableTabsPanelStack>
      </Tabs>

      {/* TOS attribution footer (ComicVine / Metron require source
       * links on every page using their data). The External IDs and
       * Covers content has moved into the Tabs row above; the footer
       * stays standalone because attribution must remain visible
       * regardless of which tab the user has open. */}
      <IssueSourcesFooter seriesSlug={seriesSlug} issueSlug={issue.slug} />
    </div>
  );
}

/**
 * Inline icon row for the issue-level facts admins ask for visibly:
 * publisher, publication date, page count, reading time, language. Each
 * cell is muted so it reads as supporting metadata, not a primary CTA.
 */
function IssueFactRow({
  issue,
  series,
  publicationDate,
  readingTime,
  lastReadAt,
  timesRead,
}: {
  issue: IssueDetailView;
  series: SeriesView | null;
  publicationDate: string | null;
  readingTime: string | null;
  lastReadAt: string | null;
  timesRead: number;
}) {
  const publisher = issue.publisher ?? series?.publisher ?? null;
  const facts: { icon: React.ReactNode; label: string }[] = [];
  if (publisher) {
    facts.push({
      icon: <Building2 className="h-4 w-4" />,
      label: publisher,
    });
  }
  if (issue.page_count) {
    facts.push({
      icon: <FileStack className="h-4 w-4" />,
      label: `${issue.page_count} pages`,
    });
  }
  if (readingTime) {
    facts.push({
      icon: <Clock className="h-4 w-4" />,
      label: `~${readingTime}`,
    });
  }
  if (publicationDate) {
    facts.push({
      icon: <Calendar className="h-4 w-4" />,
      label: publicationDate,
    });
  }
  if (issue.language_code) {
    facts.push({
      icon: <Languages className="h-4 w-4" />,
      label: issue.language_code.toUpperCase(),
    });
  }
  // The "Last read" fact sits last so the publication-time row stays
  // stable for unread issues and only grows when the user has activity.
  // `× N` suffix appears only on re-reads so single-reads stay clean.
  const lastReadLabel = formatRelativeDate(lastReadAt);
  if (lastReadLabel) {
    facts.push({
      icon: <History className="h-4 w-4" />,
      label:
        timesRead > 1
          ? `Last read ${lastReadLabel} · ${timesRead}×`
          : `Last read ${lastReadLabel}`,
    });
  }
  if (facts.length === 0) return null;
  return (
    <div className="text-muted-foreground mt-2 flex flex-wrap items-center gap-x-3 gap-y-1 text-xs sm:gap-x-4 sm:gap-y-2 sm:text-sm">
      {facts.map((f, i) => (
        <span key={i} className="inline-flex items-center gap-1.5">
          {/* Icons drop on mobile to keep the row compact and on a
              single visual line; sm+ keeps the iconed treatment. */}
          <span className="text-muted-foreground/80 hidden sm:inline">
            {f.icon}
          </span>
          <span>{f.label}</span>
        </span>
      ))}
    </div>
  );
}

/**
 * First entry of a CSV-style credits field, used by the stats grid where
 * we only have room for the headline name. Falls back to the entire
 * trimmed value if the split yields nothing.
 */
function primaryCredit(value: string | null | undefined): string | null {
  if (!value) return null;
  const first = value.split(/[,;]/)[0]?.trim();
  return first && first.length > 0 ? first : value.trim() || null;
}

/** Pretty-print a byte count as B / KB / MB / GB / TB with one decimal. */
function formatFileSize(bytes: number | null | undefined): string | null {
  if (bytes == null || !Number.isFinite(bytes) || bytes < 0) return null;
  const units = ["B", "KB", "MB", "GB", "TB"];
  let n = bytes;
  let i = 0;
  while (n >= 1024 && i < units.length - 1) {
    n /= 1024;
    i += 1;
  }
  // Bytes need no decimals; everything else gets one.
  return i === 0 ? `${n} ${units[i]}` : `${n.toFixed(1)} ${units[i]}`;
}

function ExternalTextLink({
  url,
  label,
}: {
  url: string;
  label?: string | null;
}) {
  return (
    <a
      href={url}
      target="_blank"
      rel="noreferrer"
      className="text-primary break-all hover:underline"
    >
      {label ?? url.replace(/^https?:\/\//, "")}
    </a>
  );
}

function FilePathValue({ path }: { path: string }) {
  return (
    <code className="bg-muted/50 block rounded-md px-3 py-2 font-mono text-xs break-all">
      {path}
    </code>
  );
}

const REDUNDANT_WEB_URL_SOURCES = new Set(["comicvine", "metron", "gcd"]);

function isProviderWebUrlDuplicate(
  webUrl: string | null | undefined,
  externalIds: ExternalIdRow[],
): boolean {
  if (!webUrl) return false;
  const comparableWebUrl = normalizeComparableUrl(webUrl);
  const webProviderId = extractProviderIssueIdFromUrl(webUrl);

  return externalIds.some((row) => {
    if (!REDUNDANT_WEB_URL_SOURCES.has(row.source)) return false;
    if (
      comparableWebUrl &&
      row.external_url &&
      comparableWebUrl === normalizeComparableUrl(row.external_url)
    ) {
      return true;
    }
    return (
      webProviderId?.source === row.source &&
      webProviderId.id === normalizeExternalId(row.external_id)
    );
  });
}

function normalizeComparableUrl(value: string): string | null {
  const parsed = parseComparableUrl(value);
  if (!parsed) return null;
  parsed.hash = "";
  parsed.search = "";
  parsed.protocol = "https:";
  parsed.hostname = parsed.hostname.replace(/^www\./, "").toLowerCase();
  parsed.pathname = parsed.pathname.replace(/\/+$/, "");
  return parsed.toString();
}

function parseComparableUrl(value: string): URL | null {
  const trimmed = value.trim();
  if (!trimmed) return null;
  try {
    return new URL(trimmed);
  } catch {
    try {
      return new URL(`https://${trimmed}`);
    } catch {
      return null;
    }
  }
}

function extractProviderIssueIdFromUrl(
  value: string,
): { source: string; id: string } | null {
  const parsed = parseComparableUrl(value);
  if (!parsed) return null;
  const host = parsed.hostname.replace(/^www\./, "").toLowerCase();
  const segments = parsed.pathname
    .split("/")
    .map((segment) => segment.trim())
    .filter(Boolean);

  if (host === "comicvine.gamespot.com") {
    for (const segment of segments) {
      const match = /^4000-(\d+)$/i.exec(segment);
      if (match) return { source: "comicvine", id: match[1] };
    }
    return null;
  }

  if (host === "metron.cloud") {
    return extractPathScopedId("metron", "issue", segments);
  }

  if (host === "comics.org") {
    return extractPathScopedId("gcd", "issue", segments);
  }

  return null;
}

function extractPathScopedId(
  source: string,
  scope: string,
  segments: string[],
): { source: string; id: string } | null {
  const scopeIndex = segments.findIndex(
    (segment) => segment.toLowerCase() === scope,
  );
  const id = scopeIndex >= 0 ? segments[scopeIndex + 1] : null;
  return id ? { source, id: normalizeExternalId(id) } : null;
}

function normalizeExternalId(value: string): string {
  return value.trim().replace(/\/+$/, "").toLowerCase();
}

function splitCsv(value: string | null | undefined): string[] {
  if (!value) return [];
  // Mirrors `server::library::scanner::metadata_rollup::split_csv`. If
  // `;` is present anywhere in the string we treat it as the sole
  // separator so names containing commas (e.g. `"Capes, Inc."`) survive
  // the round-trip; otherwise we split on `,` as before. Dedupe is
  // case-insensitive, first casing wins.
  const sep = value.includes(";") ? ";" : ",";
  const seen = new Set<string>();
  const out: string[] = [];
  for (const piece of value.split(sep)) {
    const trimmed = piece.trim();
    if (!trimmed) continue;
    const key = trimmed.toLowerCase();
    if (seen.has(key)) continue;
    seen.add(key);
    out.push(trimmed);
  }
  return out;
}

function hasAnyCredit(issue: IssueDetailView): boolean {
  return Boolean(
    issue.writer ||
    issue.penciller ||
    issue.inker ||
    issue.colorist ||
    issue.letterer ||
    issue.cover_artist,
  );
}

/**
 * "More in series" rail — the previous issue is a labeled convenience slot,
 * then the up-next sequence continues on the same horizontal scroller.
 */
function NextInSeries({
  prev,
  items,
  series,
}: {
  /** The single issue before the current one, shown as the leftmost cover.
   *  Null on the first issue of the series — then the rail starts at the
   *  next issues. */
  prev: IssueSummaryView | null;
  items: IssueSummaryView[];
  series: SeriesView;
}) {
  return (
    <section className="space-y-3">
      <h2 className="text-base font-semibold tracking-tight">More in series</h2>
      <HorizontalScrollRail className="-mx-1">
        {prev && (
          <div className="w-36 shrink-0 space-y-2 sm:w-40">
            <p className="text-muted-foreground flex items-center gap-1 text-[11px] font-semibold tracking-wide uppercase">
              <ArrowLeft aria-hidden="true" className="size-3" />
              Previous issue
            </p>
            <MoreInSeriesIssueCard issue={prev} />
          </div>
        )}

        {items.map((it, index) => (
          <div key={it.id} className="w-36 shrink-0 space-y-2 sm:w-40">
            <p
              aria-hidden={index === 0 ? undefined : "true"}
              className={cn(
                "text-foreground flex items-center gap-1 text-[11px] font-semibold tracking-wide uppercase",
                index > 0 && "invisible",
              )}
            >
              <ArrowRight aria-hidden="true" className="size-3" />
              Up next
            </p>
            <MoreInSeriesIssueCard issue={it} />
          </div>
        ))}

        <div className="w-36 shrink-0 space-y-2 sm:w-40">
          <p
            aria-hidden="true"
            className="invisible flex items-center gap-1 text-[11px] font-semibold tracking-wide uppercase"
          >
            View all
          </p>
          <div className="flex aspect-[2/3] items-center justify-center">
            <Button asChild variant="outline" size="sm" className="shrink-0">
              <Link
                href={seriesUrl(series)}
                aria-label="View all issues in series"
              >
                <span>View all</span>
                <ArrowRight aria-hidden="true" />
              </Link>
            </Button>
          </div>
        </div>
      </HorizontalScrollRail>
    </section>
  );
}

function MoreInSeriesIssueCard({ issue }: { issue: IssueSummaryView }) {
  return (
    <Link
      href={issueUrl(issue)}
      className="group block min-w-0 space-y-1.5 rounded-md focus-visible:outline-none"
    >
      <Cover
        src={issue.cover_url}
        alt={issue.title ?? `Issue ${issue.number ?? ""}`}
        fallback={issue.state === "active" ? "Cover" : issue.state}
        className="group-focus-visible:ring-ring transition duration-200 group-hover:scale-[1.02] group-focus-visible:ring-2"
      />
      <div className="min-w-0">
        <p className="text-foreground truncate text-xs font-medium">
          {issue.number ? `#${issue.number}` : "—"}
          {issue.title ? ` · ${issue.title}` : ""}
        </p>
        {issue.year != null && (
          <p className="text-muted-foreground text-[11px]">{issue.year}</p>
        )}
      </div>
    </Link>
  );
}
