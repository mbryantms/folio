"use client";

import * as React from "react";
import Link from "next/link";
import { CalendarDays, Images, Rows3 } from "lucide-react";

import { PageHeader } from "@/components/admin/PageHeader";
import { EmptyState } from "@/components/folio/empty-state";
import { FilterPill } from "@/components/folio/filter-pill";
import { InfiniteSentinel } from "@/components/folio/infinite-sentinel";
import { NativeSelect } from "@/components/folio/native-select";
import {
  Sheet,
  SheetContent,
  SheetHeader,
  SheetTitle,
} from "@/components/folio/sheet";
import {
  CoverSizeFields,
  CoverSizeOptions,
} from "@/components/library/CoverSizeOptions";
import { PhoneControlsBar } from "@/components/library/PhoneControlsBar";
import { StickyToolbarRow } from "@/components/library/StickyToolbarRow";
import { RailError } from "@/components/saved-views/RailError";
import { SegmentedControl } from "@/components/settings/SegmentedControl";
import { CoverGridSkeleton } from "@/components/skeletons/LoadingShapes";
import { Button } from "@/components/ui/button";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { UpcomingCard } from "@/components/upcoming/UpcomingCard";
import { UpcomingEntryDialog } from "@/components/upcoming/UpcomingEntryDialog";
import { UpcomingList } from "@/components/upcoming/UpcomingList";
import { useUpcomingInfinite, useUpcomingSummary } from "@/lib/api/queries";
import { useMe } from "@/lib/api/queries-me";
import type { UpcomingFilters } from "@/lib/api/query-keys";
import type {
  UpcomingEntryView,
  UpcomingScope,
  UpcomingSummaryView,
  UpcomingWeekView,
} from "@/lib/api/types";
import { COVER_GRID_STYLE } from "@/lib/library/cover-size";
import { timeAgo } from "@/lib/sessions";
import { statusToneText } from "@/lib/ui/status-tone";
import { type UpcomingLayout, useUpcomingLayout } from "@/lib/upcoming/layout";
import { weekHeading, weekRange } from "@/lib/upcoming/stage";
import { useLocalToday } from "@/lib/upcoming/today";
import { cn } from "@/lib/utils";

const SCOPE_OPTIONS: ReadonlyArray<{ value: UpcomingScope; label: string }> = [
  { value: "mine", label: "My series" },
  { value: "all", label: "All releases" },
];

const LAYOUT_OPTIONS: ReadonlyArray<{ value: UpcomingLayout; label: string }> =
  [
    { value: "grid", label: "Covers" },
    { value: "list", label: "List" },
  ];

/**
 * `/upcoming`: what is coming out, week by week, from Metron's release
 * calendar, with what Mylar reported about each issue for readers allowed
 * to see it (upcoming-releases 1.0).
 *
 * View only. Every control here narrows or lays out the list; none of
 * them, and nothing in Folio, changes anything in Mylar.
 *
 * Every filter is a server query param (`useUpcomingInfinite`), and weeks
 * load a page at a time behind the sentinel, so the page never filters or
 * truncates in memory.
 *
 * Layout (docs/dev/phone-layout.md): the filters are tools you need
 * while scrolling, so the control row pins: `StickyToolbarRow` from
 * `sm` up, `PhoneControlsBar` below, where the title rides in the bar and
 * the header is hidden. Week headings are plain headings.
 */
export function UpcomingView() {
  const today = useLocalToday();
  const isAdmin = useMe().data?.role === "admin";
  const [scope, setScope] = React.useState<UpcomingScope>("mine");
  const [tracked, setTracked] = React.useState(false);
  const [inLibrary, setInLibrary] = React.useState(false);
  const [firstIssues, setFirstIssues] = React.useState(false);
  const [publisher, setPublisher] = React.useState("");
  const [layout, setLayout] = useUpcomingLayout();
  const [filtersOpen, setFiltersOpen] = React.useState(false);
  const [selected, setSelected] = React.useState<UpcomingEntryView | null>(
    null,
  );
  const [dialogOpen, setDialogOpen] = React.useState(false);

  const summaryQ = useUpcomingSummary(today ?? "", {
    enabled: today !== null,
  });
  const summary = summaryQ.data;
  // A viewer who may not see Mylar data never gets a tracked release, and
  // one held to their own series has nothing "All releases" would add.
  const mylarVisible = Boolean(summary?.mylar);
  const canSeeAll = summary?.all_releases_available ?? false;
  const effectiveScope: UpcomingScope = canSeeAll ? scope : "mine";
  // "In library" only narrows a list that can hold something else.
  const showInLibrary = effectiveScope === "all" || mylarVisible;
  const publishers = summary?.publishers ?? [];

  const filters = React.useMemo<UpcomingFilters>(
    () => ({
      today: today ?? "",
      scope: effectiveScope,
      publisher: publisher || undefined,
      tracked: (tracked && mylarVisible) || undefined,
      inLibrary: (inLibrary && showInLibrary) || undefined,
      firstIssues: firstIssues || undefined,
    }),
    [
      today,
      effectiveScope,
      publisher,
      tracked,
      mylarVisible,
      inLibrary,
      showInLibrary,
      firstIssues,
    ],
  );
  const listQ = useUpcomingInfinite(filters, {
    enabled: today !== null && summary?.enabled === true,
  });
  const weeks = React.useMemo(
    () => listQ.data?.pages.flatMap((p) => p.items) ?? [],
    [listQ.data],
  );
  const total = listQ.data?.pages[0]?.total ?? null;

  const filterCount =
    (filters.tracked ? 1 : 0) +
    (filters.inLibrary ? 1 : 0) +
    (filters.firstIssues ? 1 : 0) +
    (filters.publisher ? 1 : 0);
  const clearFilters = () => {
    setTracked(false);
    setInLibrary(false);
    setFirstIssues(false);
    setPublisher("");
  };
  const open = (entry: UpcomingEntryView) => {
    setSelected(entry);
    setDialogOpen(true);
  };

  const pills = (
    <>
      {mylarVisible ? (
        <FilterPill active={tracked} onClick={() => setTracked((v) => !v)}>
          Tracked
        </FilterPill>
      ) : null}
      {showInLibrary ? (
        <FilterPill active={inLibrary} onClick={() => setInLibrary((v) => !v)}>
          In library
        </FilterPill>
      ) : null}
      <FilterPill
        active={firstIssues}
        onClick={() => setFirstIssues((v) => !v)}
      >
        New #1s
      </FilterPill>
    </>
  );
  const publisherOptions = [
    { value: "", label: "All publishers" },
    ...publishers.map((name) => ({ value: name, label: name })),
  ];

  const off = summary?.enabled === false;
  const loading =
    today === null || summaryQ.isLoading || (!off && listQ.isLoading);

  return (
    <div className="space-y-6">
      <PageHeader
        title="Upcoming"
        description={
          mylarVisible
            ? "New issues by week, from Metron's release calendar, with what Mylar is doing about each one."
            : "New issues by week, from Metron's release calendar."
        }
        phoneDescription="hidden"
        // Below `sm` the title rides in the PhoneControlsBar instead.
        className="max-sm:hidden"
      />

      {off ? null : (
        <>
          <StickyToolbarRow>
            {canSeeAll ? (
              <SegmentedControl
                value={scope}
                onChange={setScope}
                options={SCOPE_OPTIONS}
                ariaLabel="Which releases"
                className="h-9"
              />
            ) : null}
            <div
              role="group"
              aria-label="Filters"
              className="flex flex-wrap items-center gap-1"
            >
              {pills}
            </div>
            {publishers.length > 0 ? (
              <NativeSelect
                aria-label="Publisher"
                value={publisher}
                onChange={setPublisher}
                options={publisherOptions}
                className="folio-control w-48"
              />
            ) : null}
            <div className="ml-auto flex items-center gap-2">
              <ToggleGroup
                type="single"
                variant="outline"
                spacing={0}
                value={layout}
                onValueChange={(v) => {
                  if (v === "grid" || v === "list") setLayout(v);
                }}
                aria-label="Layout"
              >
                {/* Not the four-squares glyph: that one is View options
                    (phone-layout rule 2), and it sits right beside. */}
                <ToggleGroupItem
                  value="grid"
                  aria-label="Covers"
                  title="Covers"
                >
                  <Images aria-hidden="true" />
                </ToggleGroupItem>
                <ToggleGroupItem value="list" aria-label="List" title="List">
                  <Rows3 aria-hidden="true" />
                </ToggleGroupItem>
              </ToggleGroup>
              {layout === "grid" ? (
                <CoverSizeOptions fieldId="upcoming-cover-size" />
              ) : null}
            </div>
          </StickyToolbarRow>

          <PhoneControlsBar
            pageTop
            leading={
              <h1 className="min-w-0 truncate text-xl font-semibold tracking-tight">
                Upcoming
              </h1>
            }
            filters={{ count: filterCount, onOpen: () => setFiltersOpen(true) }}
            menu={() => (
              <>
                <div className="space-y-2">
                  <p className="text-muted-foreground text-xs font-medium tracking-wide uppercase">
                    Layout
                  </p>
                  <SegmentedControl
                    value={layout}
                    onChange={setLayout}
                    options={LAYOUT_OPTIONS}
                    ariaLabel="Layout"
                    fill
                  />
                </div>
                {layout === "grid" ? (
                  <CoverSizeFields fieldId="upcoming-phone-cover-size" />
                ) : null}
              </>
            )}
          />
          <Sheet open={filtersOpen} onOpenChange={setFiltersOpen}>
            <SheetContent
              side="bottom"
              className="rounded-t-lg pb-[max(1.5rem,var(--safe-bottom))]"
            >
              <SheetHeader>
                <SheetTitle>Filters</SheetTitle>
              </SheetHeader>
              <div className="space-y-5">
                <div className="flex flex-wrap gap-1.5">{pills}</div>
                {publishers.length > 0 ? (
                  <div className="space-y-2">
                    <p className="text-muted-foreground text-xs font-medium tracking-wide uppercase">
                      Publisher
                    </p>
                    <NativeSelect
                      aria-label="Publisher"
                      value={publisher}
                      onChange={setPublisher}
                      options={publisherOptions}
                      className="folio-control"
                    />
                  </div>
                ) : null}
                <Button
                  type="button"
                  className="h-11 w-full"
                  onClick={() => setFiltersOpen(false)}
                >
                  Done
                </Button>
              </div>
            </SheetContent>
          </Sheet>

          {/* Phone: the scope toggle is a one-time choice, so it sits in
              flow under the bar and scrolls away with the page. */}
          {canSeeAll ? (
            <SegmentedControl
              value={scope}
              onChange={setScope}
              options={SCOPE_OPTIONS}
              ariaLabel="Which releases"
              fill
              className="sm:hidden"
            />
          ) : null}

          <SourceLine summary={summary} total={loading ? null : total} />
        </>
      )}

      {loading ? (
        <CoverGridSkeleton count={12} label="Loading upcoming releases…" />
      ) : summaryQ.isError ? (
        <RailError
          message="Couldn't load upcoming releases."
          onRetry={() => void summaryQ.refetch()}
        />
      ) : off ? (
        <EmptyState
          icon={CalendarDays}
          title="Upcoming releases are turned off"
          description={
            isAdmin
              ? "Turn the release calendar on to see what is coming out."
              : "An administrator has not turned the release calendar on."
          }
          action={
            isAdmin ? (
              <Button asChild>
                <Link href="/admin/upcoming">Open Upcoming settings</Link>
              </Button>
            ) : undefined
          }
        />
      ) : listQ.isError && !listQ.data ? (
        <RailError
          message="Couldn't load upcoming releases."
          onRetry={() => void listQ.refetch()}
        />
      ) : weeks.length === 0 ? (
        <NothingUpcoming
          neverSynced={!summary?.calendar_synced_at}
          filtered={filterCount > 0}
          scope={effectiveScope}
          canSeeAll={canSeeAll}
          isAdmin={isAdmin}
          onClearFilters={clearFilters}
          onShowAll={() => setScope("all")}
        />
      ) : (
        <div className="space-y-10">
          {weeks.map((week) => (
            <WeekSection
              key={week.week_start}
              week={week}
              today={today ?? week.week_start}
              layout={layout}
              onOpen={open}
            />
          ))}
          <InfiniteSentinel query={listQ} label="Loading more weeks…" />
        </div>
      )}

      <UpcomingEntryDialog
        entry={selected}
        open={dialogOpen}
        onOpenChange={setDialogOpen}
      />
    </div>
  );
}

/** How many releases match, and how old each source is. Mylar's age
 *  turns to the warning tone once the snapshot is stale: the statuses on
 *  the page are then as of that time, not now. */
function SourceLine({
  summary,
  total,
}: {
  summary: UpcomingSummaryView | undefined;
  total: number | null;
}) {
  if (!summary?.enabled) return null;
  const mylar = summary.mylar;
  return (
    <p className="text-muted-foreground flex flex-wrap gap-x-3 gap-y-1 text-xs">
      {total !== null ? (
        <span data-testid="upcoming-total">
          {total} {total === 1 ? "release" : "releases"}
        </span>
      ) : null}
      {summary.calendar_synced_at ? (
        <span>Calendar updated {timeAgo(summary.calendar_synced_at)}</span>
      ) : null}
      {mylar ? (
        <span
          className={cn(
            mylar.freshness === "stale" && statusToneText("warning"),
          )}
        >
          {mylar.freshness === "stale"
            ? `Mylar last reported ${timeAgo(mylar.received_at)}`
            : `Mylar updated ${timeAgo(mylar.received_at)}`}
        </span>
      ) : null}
    </p>
  );
}

function WeekSection({
  week,
  today,
  layout,
  onOpen,
}: {
  week: UpcomingWeekView;
  today: string;
  layout: UpcomingLayout;
  onOpen: (entry: UpcomingEntryView) => void;
}) {
  const id = `upcoming-week-${week.week_start}`;
  const heading = weekHeading(week.week_start, today);
  const count = week.entries.length;
  return (
    <section aria-labelledby={id} className="space-y-3">
      <div className="flex flex-wrap items-baseline gap-x-3">
        <h2 id={id} className="text-lg font-semibold tracking-tight">
          {heading}
        </h2>
        <p className="text-muted-foreground text-sm">
          {weekRange(week.week_start, week.week_end)} · {count}{" "}
          {count === 1 ? "release" : "releases"}
        </p>
      </div>
      {layout === "grid" ? (
        <ul className="grid gap-4" style={COVER_GRID_STYLE}>
          {week.entries.map((entry) => (
            <li key={entry.key}>
              <UpcomingCard entry={entry} onOpen={onOpen} />
            </li>
          ))}
        </ul>
      ) : (
        <UpcomingList
          entries={week.entries}
          today={today}
          caption={heading}
          onOpen={onOpen}
        />
      )}
    </section>
  );
}

function NothingUpcoming({
  neverSynced,
  filtered,
  scope,
  canSeeAll,
  isAdmin,
  onClearFilters,
  onShowAll,
}: {
  neverSynced: boolean;
  filtered: boolean;
  scope: UpcomingScope;
  canSeeAll: boolean;
  isAdmin: boolean;
  onClearFilters: () => void;
  onShowAll: () => void;
}) {
  if (neverSynced) {
    return (
      <EmptyState
        icon={CalendarDays}
        title="The release calendar is not filled yet"
        description="Folio fetches upcoming releases from Metron in the background. The first pass takes a few minutes."
        action={
          isAdmin ? (
            <Button variant="outline" asChild>
              <Link href="/admin/upcoming">Check the calendar status</Link>
            </Button>
          ) : undefined
        }
      />
    );
  }
  if (filtered) {
    return (
      <EmptyState
        icon={CalendarDays}
        title="No releases match these filters"
        action={
          <Button variant="outline" onClick={onClearFilters}>
            Clear filters
          </Button>
        }
      />
    );
  }
  if (scope === "mine") {
    return (
      <EmptyState
        icon={CalendarDays}
        title="Nothing upcoming in your series"
        description="No series in your library has a new issue announced for the coming weeks."
        action={
          canSeeAll ? (
            <Button variant="outline" onClick={onShowAll}>
              Show all releases
            </Button>
          ) : undefined
        }
      />
    );
  }
  return (
    <EmptyState
      icon={CalendarDays}
      title="Nothing upcoming"
      description="The release calendar has no issues for the coming weeks."
    />
  );
}
