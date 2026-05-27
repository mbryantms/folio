import {
  ArrowRight,
  Building2,
  Calendar,
  ChevronLeft,
  ChevronRight,
  Clock,
  FileStack,
  History,
  Languages,
} from "lucide-react";
import Link from "next/link";
import { notFound, redirect } from "next/navigation";

import { issueUrl, readerUrl, seriesUrl } from "@/lib/urls";
import { IssueActivityTab } from "@/components/activity/IssueActivityTab";
import { Cover } from "@/components/Cover";
import { ChipList } from "@/components/library/ChipList";
import { Description } from "@/components/library/Description";
import { IssueHealthBadge } from "@/components/library/IssueHealthBadge";
import { CoverGallery } from "@/components/library/CoverGallery";
import { ExternalIdsCard } from "@/components/library/ExternalIdsCard";
import { ProviderBadgesRow } from "@/components/library/ProviderBadgesRow";

import { IssueSourcesFooter } from "./IssueMetadataPanel";
import { MetadataGrid } from "@/components/library/MetadataGrid";
import { Stat } from "@/components/library/Stat";
import { UserRating } from "@/components/library/UserRating";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { apiGet, ApiError } from "@/lib/api/fetch";
import type {
  IssueDetailView,
  IssueSummaryView,
  NextInSeriesView,
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
import {
  type ProgressLike,
  type ReadState,
  readButtonLabel,
  readStateFor,
} from "@/lib/reading-state";

import { InlineNotesEditor } from "./InlineNotesEditor";
import { IssueActions } from "./IssueActions";

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
  // Best-effort series lookup for breadcrumb + status badge.
  let series: SeriesView | null = null;
  try {
    series = await apiGet<SeriesView>(`/series/${seriesSlug}`);
  } catch {
    /* fallthrough */
  }

  // Best-effort progress fetch — drives the dynamic read CTA label.
  // 4xx (e.g. unauthenticated) just means "no record yet, show Read".
  let issueProgress: ProgressLike | null = null;
  try {
    const delta = await apiGet<{ records: ProgressLike[] }>(`/progress`);
    issueProgress = delta.records.find((r) => r.issue_id === issue.id) ?? null;
  } catch {
    /* no progress yet */
  }

  // Issue-scoped reading stats — drives the inline "Last read" fact
  // line, the prominent activity strip above the tabs, and gates the
  // dedicated Activity tab. One server fetch hydrates all three so
  // the page never has to wait on a client roundtrip to know whether
  // there's anything to show.
  let activityStats: ReadingStatsView | null = null;
  try {
    activityStats = await apiGet<ReadingStatsView>(
      `/me/reading-stats?range=all&issue_id=${encodeURIComponent(issue.id)}`,
    );
  } catch {
    /* leave null — page degrades to the no-activity layout */
  }
  const hasActivity = (activityStats?.totals.sessions ?? 0) > 0;

  // Next 5 issues in the series, ordered by sort_number. Best-effort —
  // a transient failure simply hides the section.
  let nextIssues: IssueSummaryView[] = [];
  try {
    const next = await apiGet<NextInSeriesView>(
      `/series/${seriesSlug}/issues/${issueSlug}/next?limit=5`,
    );
    nextIssues = next.items;
  } catch {
    /* hide section on failure */
  }
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

      {nextIssues.length > 0 && series && (
        <NextInSeries items={nextIssues} series={series} />
      )}

      <Tabs defaultValue="details">
        <TabsList>
          <TabsTrigger value="details">Details</TabsTrigger>
          <TabsTrigger value="credits">Credits</TabsTrigger>
          <TabsTrigger value="cast">Cast &amp; Setting</TabsTrigger>
          <TabsTrigger value="genres">Genres &amp; Tags</TabsTrigger>
          <TabsTrigger value="covers">Covers</TabsTrigger>
          <TabsTrigger value="external-ids">External IDs</TabsTrigger>
          <TabsTrigger value="notes">Notes</TabsTrigger>
          {hasActivity && <TabsTrigger value="activity">Activity</TabsTrigger>}
        </TabsList>

        {/* Static-metadata tabs are `forceMount`-ed and stacked in a single
         * grid cell. The cell sizes to the tallest tab so switching between
         * Details (long) and Genres (short) no longer shrinks the document
         * height — preventing the page-jump that happens when the browser
         * clamps `scrollTop` to a smaller scroll range. Activity is left
         * out of the stack on purpose: it triggers `useReadingStats` /
         * `useReadingSessions` on mount, so we keep it on-demand. */}
        <div className="grid">
          <TabsContent
            forceMount
            value="details"
            className="col-start-1 row-start-1 pt-6 data-[state=inactive]:pointer-events-none data-[state=inactive]:invisible"
          >
            <MetadataGrid
              items={[
                { label: "Series", value: series?.name },
                {
                  label: "Issue number",
                  value: issue.number ?? null,
                },
                {
                  label: "Sort order",
                  value:
                    issue.sort_number != null
                      ? issue.sort_number.toString()
                      : null,
                },
                { label: "Volume", value: issue.volume ?? series?.volume ?? null },
                { label: "Publication date", value: publicationDate },
                {
                  label: "Publication status",
                  value: seriesStatus,
                },
                {
                  label: "Language",
                  value: issue.language_code?.toUpperCase(),
                },
                { label: "Age rating", value: issue.age_rating },
                { label: "Format", value: issue.format },
                {
                  label: "Black & white",
                  value:
                    issue.black_and_white == null
                      ? null
                      : issue.black_and_white
                        ? "Yes"
                        : "No",
                },
                { label: "Manga", value: issue.manga },
                { label: "Publisher", value: issue.publisher },
                { label: "Imprint", value: issue.imprint },
                { label: "Story arc", value: issue.story_arc },
                { label: "Story arc number", value: issue.story_arc_number },
                { label: "GTIN", value: issue.gtin },
                // ComicVine ID + Metron ID intentionally absent — they
                // live in the "External IDs" tab alongside every other
                // provider identifier. Surfacing them twice was confusing
                // (the Details grid showed two while the panel below
                // showed N including those two).
                { label: "Pages", value: formatPageCount(issue.page_count) },
                {
                  label: "Reading time",
                  value: readingTime ? `≈ ${readingTime}` : null,
                },
                { label: "Added", value: formatRelativeDate(issue.created_at) },
                {
                  label: "Updated",
                  value: formatRelativeDate(issue.updated_at),
                },
                {
                  label: "File",
                  value: (
                    <span className="font-mono text-xs break-all">
                      {issue.file_path}
                    </span>
                  ),
                  wide: true,
                },
                { label: "File size", value: formatFileSize(issue.file_size) },
                // External links row removed — provider IDs live in the
                // External IDs tab and link out from there; the curated
                // additional_links live in the Edit sheet. A second
                // copy here was just duplication.
              ]}
            />
            {/* "Locally edited fields" summary moved into the Edit sheet
                — fields surface a per-row release control alongside their
                input, so the user can both see what's pinned and release
                it without leaving the editor. */}
          </TabsContent>

          <TabsContent
            forceMount
            value="credits"
            className="col-start-1 row-start-1 pt-6 data-[state=inactive]:pointer-events-none data-[state=inactive]:invisible"
          >
            <div className="grid gap-6 sm:grid-cols-2">
              <ChipList
                label="Writer"
                items={splitCsv(issue.writer)}
                filterField="writer"
                creatorSlugs={issue.creator_slugs}
              />
              <ChipList
                label="Penciller"
                items={splitCsv(issue.penciller)}
                filterField="penciller"
                creatorSlugs={issue.creator_slugs}
              />
              <ChipList
                label="Inker"
                items={splitCsv(issue.inker)}
                filterField="inker"
                creatorSlugs={issue.creator_slugs}
              />
              <ChipList
                label="Colorist"
                items={splitCsv(issue.colorist)}
                filterField="colorist"
                creatorSlugs={issue.creator_slugs}
              />
              <ChipList
                label="Letterer"
                items={splitCsv(issue.letterer)}
                filterField="letterer"
                creatorSlugs={issue.creator_slugs}
              />
              <ChipList
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
          </TabsContent>

          <TabsContent
            forceMount
            value="cast"
            className="col-start-1 row-start-1 pt-6 data-[state=inactive]:pointer-events-none data-[state=inactive]:invisible"
          >
            <div className="grid gap-6 sm:grid-cols-2">
              <ChipList
                label="Characters"
                items={splitCsv(issue.characters)}
                filterField="characters"
              />
              <ChipList
                label="Teams"
                items={splitCsv(issue.teams)}
                filterField="teams"
              />
              <ChipList
                label="Locations"
                items={splitCsv(issue.locations)}
                filterField="locations"
              />
              {/* Story arc has no series-level library filter (it's an
                  issue-level concept), so the chip stays read-only. */}
              <ChipList label="Story arc" items={splitCsv(issue.story_arc)} />
            </div>
            {!issue.characters &&
              !issue.teams &&
              !issue.locations &&
              !issue.story_arc && (
                <p className="text-muted-foreground text-sm">
                  No cast or setting metadata.
                </p>
              )}
          </TabsContent>

          <TabsContent
            forceMount
            value="genres"
            className="col-start-1 row-start-1 pt-6 data-[state=inactive]:pointer-events-none data-[state=inactive]:invisible"
          >
            <div className="grid gap-6 sm:grid-cols-2">
              <ChipList
                label="Genres"
                items={splitCsv(issue.genre)}
                filterField="genres"
              />
              <ChipList
                label="Tags"
                items={splitCsv(issue.tags)}
                filterField="tags"
              />
            </div>
            {!issue.genre && !issue.tags && (
              <p className="text-muted-foreground text-sm">
                No genres or tags.
              </p>
            )}
          </TabsContent>

          <TabsContent
            forceMount
            value="external-ids"
            className="col-start-1 row-start-1 pt-6 data-[state=inactive]:pointer-events-none data-[state=inactive]:invisible"
          >
            <ExternalIdsCard
              entityType="issue"
              seriesSlug={seriesSlug}
              issueSlug={issue.slug}
              chrome="bare"
            />
          </TabsContent>

          <TabsContent
            forceMount
            value="notes"
            className="col-start-1 row-start-1 pt-6 data-[state=inactive]:pointer-events-none data-[state=inactive]:invisible"
          >
            <InlineNotesEditor
              seriesSlug={seriesSlug}
              issueSlug={issue.slug}
              initial={issue.notes ?? null}
            />
          </TabsContent>
          {/* Covers tab is intentionally OUTSIDE the forceMount stack:
            * variant cover tiles are tall, and pinning them into the
            * stack would force every other tab's panel to that height
            * (the "lots of empty space at the bottom" effect). On-demand
            * mount also matches Activity, which stays out of the stack
            * for its own perf reason. */}
          <TabsContent value="covers" className="col-start-1 row-start-1 pt-6">
            <CoverGallery issueId={issue.id} chrome="bare" />
          </TabsContent>
          {hasActivity && (
            <TabsContent
              value="activity"
              className="col-start-1 row-start-1 pt-6"
            >
              <IssueActivityTab
                issueId={issue.id}
                pageCount={issue.page_count ?? null}
              />
            </TabsContent>
          )}
        </div>
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
 * "Next in series" carousel — the next N issues by sort_number, followed by
 * a "view all in series" tile that lives in the same grid slot as a cover
 * card. The tile sits flush against the last issue on wide screens so the
 * affordance doesn't drift to the far right edge of the section.
 */
function NextInSeries({
  items,
  series,
}: {
  items: IssueSummaryView[];
  series: SeriesView;
}) {
  return (
    <section className="space-y-3">
      <h2 className="text-base font-semibold tracking-tight">Next in series</h2>
      <ul
        className="grid gap-4"
        style={{
          gridTemplateColumns: "repeat(auto-fill, minmax(7.5rem, 1fr))",
        }}
      >
        {items.map((it) => (
          <li key={it.id}>
            <Link
              href={issueUrl(it)}
              className="group block space-y-1.5 focus-visible:outline-none"
            >
              <Cover
                src={it.cover_url}
                alt={it.title ?? `Issue ${it.number ?? ""}`}
                fallback={it.state === "active" ? "Cover" : it.state}
                className="group-focus-visible:ring-ring transition-transform duration-200 group-hover:scale-[1.02] group-focus-visible:ring-2"
              />
              <div className="min-w-0">
                <p className="text-foreground truncate text-xs font-medium">
                  {it.number ? `#${it.number}` : "—"}
                  {it.title ? ` · ${it.title}` : ""}
                </p>
                {it.year != null && (
                  <p className="text-muted-foreground text-[11px]">{it.year}</p>
                )}
              </div>
            </Link>
          </li>
        ))}
        {/* Sits in the same `auto-fill` slot as the trailing issue card, but
         * styled as a small shadcn outline button vertically centered in the
         * cell so it doesn't masquerade as another issue. */}
        <li className="flex items-center">
          <Button asChild variant="outline" size="sm" className="shrink-0">
            <Link
              href={seriesUrl(series)}
              aria-label="View all issues in series"
            >
              <span>View all</span>
              <ArrowRight aria-hidden="true" />
            </Link>
          </Button>
        </li>
      </ul>
    </section>
  );
}
