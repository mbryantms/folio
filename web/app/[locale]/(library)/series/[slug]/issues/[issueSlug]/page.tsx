import {
  ArrowRight,
  Building2,
  Calendar,
  ChevronRight,
  Clock,
  FileStack,
  Languages,
} from "lucide-react";
import Link from "next/link";
import { notFound, redirect } from "next/navigation";

import { issueUrl, readerUrl, seriesUrl } from "@/lib/urls";
import { IssueActivityTab } from "@/components/activity/IssueActivityTab";
import { Cover } from "@/components/Cover";
import { ChipList } from "@/components/library/ChipList";
import { Description } from "@/components/library/Description";
import { MetadataGrid } from "@/components/library/MetadataGrid";
import { Stat } from "@/components/library/Stat";
import { UserRating } from "@/components/library/UserRating";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { apiGet, ApiError } from "@/lib/api/fetch";
import type {
  IssueDetailView,
  IssueLink,
  IssueSummaryView,
  NextInSeriesView,
  ReadingSessionListView,
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
  const cblSavedViewId =
    typeof cbl === "string" && cbl.length > 0 ? cbl : null;
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

  // Activity-tab visibility: hide the tab entirely when the user has no
  // sessions for this issue. One cheap `?limit=1` round-trip beats
  // permanently leaving an empty tab on every issue page.
  let hasActivity = false;
  try {
    const list = await apiGet<ReadingSessionListView>(
      `/me/reading-sessions?issue_id=${encodeURIComponent(issue.id)}&limit=1`,
    );
    hasActivity = list.records.length > 0;
  } catch {
    /* default false */
  }

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
      <nav
        aria-label="Breadcrumb"
        className="text-muted-foreground flex flex-wrap items-center gap-1.5 text-xs"
      >
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
      </nav>

      <header className="grid grid-cols-1 gap-6 sm:gap-8 lg:grid-cols-[18rem_1fr]">
        {/* Mobile: cover shrinks to ~44 (176px) to leave room for the
            primary CTA + the title block above the fold. Scales up to
            56 (224px) at sm, full 72 (288px) at lg. */}
        <div className="flex flex-col gap-3 sm:gap-4">
          <div className="mx-auto w-44 max-w-full sm:w-56 lg:mx-0 lg:w-72">
            <Cover
              src={
                issue.state === "active"
                  ? `/api/issues/${issue.id}/pages/0/thumb`
                  : null
              }
              alt={heading}
              fallback={issue.state === "active" ? "Cover" : issue.state}
            />
          </div>
          <div className="mx-auto flex w-full max-w-xs flex-col gap-2 sm:max-w-sm lg:mx-0 lg:max-w-72">
            {issue.state === "active" ? (
              <Button asChild size="lg" className="w-full">
                <Link href={readerUrl(issue, { cbl: cblSavedViewId })}>
                  {readLabel}
                </Link>
              </Button>
            ) : (
              <p className="border-border text-muted-foreground rounded-md border border-dashed px-4 py-2 text-center text-xs">
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
              {issue.volume != null && <span>Vol. {issue.volume}</span>}
            </div>
            <h1 className="mt-2 text-3xl font-semibold tracking-tight sm:text-4xl">
              {heading}
            </h1>
            <IssueFactRow
              issue={issue}
              series={series}
              publicationDate={publicationDate}
              readingTime={readingTime}
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
              <UserRating
                scope="issue"
                seriesSlug={issue.series_slug}
                issueSlug={issue.slug}
                initial={issue.user_rating ?? null}
                label="Your rating"
                variant="inline"
              />
            </div>
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
          {issue.notes && <TabsTrigger value="notes">Notes</TabsTrigger>}
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
                { label: "Volume", value: issue.volume },
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
                {
                  label: "ComicVine ID",
                  value:
                    issue.comicvine_id != null
                      ? String(issue.comicvine_id)
                      : null,
                },
                {
                  label: "Metron ID",
                  value:
                    issue.metron_id != null ? String(issue.metron_id) : null,
                },
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
                {
                  label: "External links",
                  value: <ExternalLinks issue={issue} />,
                  wide: true,
                },
              ]}
            />
            {issue.user_edited.length > 0 && (
              <p className="text-muted-foreground mt-4 text-xs">
                Locally edited fields:{" "}
                {issue.user_edited.map(prettyFieldName).join(", ")}. The scanner
                will not overwrite these on a metadata refresh.
              </p>
            )}
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
              />
              <ChipList
                label="Penciller"
                items={splitCsv(issue.penciller)}
                filterField="penciller"
              />
              <ChipList
                label="Inker"
                items={splitCsv(issue.inker)}
                filterField="inker"
              />
              <ChipList
                label="Colorist"
                items={splitCsv(issue.colorist)}
                filterField="colorist"
              />
              <ChipList
                label="Letterer"
                items={splitCsv(issue.letterer)}
                filterField="letterer"
              />
              <ChipList
                label="Cover artist"
                items={splitCsv(issue.cover_artist)}
                filterField="cover_artist"
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

          {issue.notes && (
            <TabsContent
              forceMount
              value="notes"
              className="col-start-1 row-start-1 pt-6 data-[state=inactive]:pointer-events-none data-[state=inactive]:invisible"
            >
              <p className="text-foreground/90 max-w-prose text-sm leading-6 whitespace-pre-wrap">
                {issue.notes}
              </p>
            </TabsContent>
          )}
          {hasActivity && (
            <TabsContent
              value="activity"
              className="col-start-1 row-start-1 pt-6"
            >
              <IssueActivityTab
                issueId={issue.id}
                pageCount={issue.page_count}
              />
            </TabsContent>
          )}
        </div>
      </Tabs>
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
}: {
  issue: IssueDetailView;
  series: SeriesView | null;
  publicationDate: string | null;
  readingTime: string | null;
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
  // ComicInfo CSV fields sometimes repeat the same name within one role
  // (e.g. "Matteo Scalera, Matteo Scalera" on a cover credit). Dedupe
  // case-insensitively, first-seen casing wins — mirrors the server-side
  // `aggregate_csv` for series-level lists.
  const seen = new Set<string>();
  const out: string[] = [];
  for (const piece of value.split(/[,;]/)) {
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

/** Combine the ComicInfo `web_url` with user-curated `additional_links`
 *  into a single rendered list. The web_url retains a stable label so
 *  users can tell which entry came from ComicInfo. */
function ExternalLinks({ issue }: { issue: IssueDetailView }) {
  const items: IssueLink[] = [];
  if (issue.web_url) {
    items.push({ label: "ComicInfo Web", url: issue.web_url });
  }
  for (const l of issue.additional_links) items.push(l);
  if (items.length === 0) return null;
  return (
    <ul className="flex flex-col gap-1">
      {items.map((l, i) => (
        <li key={`${l.url}-${i}`}>
          <a
            href={l.url}
            target="_blank"
            rel="noreferrer"
            className="text-foreground underline-offset-4 hover:underline"
          >
            {l.label ?? l.url}
          </a>
          {l.label && (
            <span className="text-muted-foreground ml-2 text-xs">{l.url}</span>
          )}
        </li>
      ))}
    </ul>
  );
}

const FIELD_LABELS: Record<string, string> = {
  title: "Title",
  number_raw: "Issue number",
  volume: "Volume",
  year: "Year",
  month: "Month",
  day: "Day",
  summary: "Summary",
  notes: "Notes",
  publisher: "Publisher",
  imprint: "Imprint",
  writer: "Writer",
  penciller: "Penciller",
  inker: "Inker",
  colorist: "Colorist",
  letterer: "Letterer",
  cover_artist: "Cover artist",
  editor: "Editor",
  translator: "Translator",
  characters: "Characters",
  teams: "Teams",
  locations: "Locations",
  alternate_series: "Alternate series",
  story_arc: "Story arc",
  story_arc_number: "Story arc number",
  genre: "Genre",
  tags: "Tags",
  language_code: "Language",
  age_rating: "Age rating",
  format: "Format",
  black_and_white: "Black & white",
  manga: "Manga",
  sort_number: "Sort number",
  web_url: "Web URL",
  gtin: "GTIN",
  comicvine_id: "ComicVine ID",
  metron_id: "Metron ID",
};

function prettyFieldName(name: string): string {
  return FIELD_LABELS[name] ?? name;
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
