"use client";

import * as React from "react";
import { ChevronDown } from "lucide-react";

import {
  CREDIT_ROLES,
  RATING_MAX,
  RATING_MIN,
  RATING_STEP,
  type CreditKey,
  type CreditState,
  type LibraryGridMode,
  type MetadataCompletenessTier,
} from "@/components/library/library-grid-filters";
import { MultiSelectEditor } from "@/components/filters/value-editors/MultiSelectEditor";
import type { OptionsEndpoint } from "@/components/filters/field-registry";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { FilterPill } from "@/components/ui/filter-pill";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { PopoverPortalContainer } from "@/components/ui/popover";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from "@/components/ui/sheet";
import { Slider } from "@/components/ui/slider";

const STATUS_OPTIONS: { value: string; label: string }[] = [
  { value: "any", label: "Any status" },
  { value: "continuing", label: "Continuing" },
  { value: "ended", label: "Ended" },
  { value: "cancelled", label: "Cancelled" },
  { value: "hiatus", label: "Hiatus" },
];

/** Per-user read-state pills (series mode). Values match the server's
 *  `read_status` param + the saved-views three-state rollup. */
const READ_STATUS_OPTIONS: { value: string; label: string }[] = [
  { value: "unread", label: "Unread" },
  { value: "in_progress", label: "Reading" },
  { value: "read", label: "Read" },
];

/** Metadata-completeness tiers (series mode). `any` is the sentinel for
 *  "unfiltered"; the rest map to the server's `metadata_completeness`
 *  param. Worklist-first ordering — "Needs metadata" sits up top. */
const METADATA_COMPLETENESS_OPTIONS: { value: string; label: string }[] = [
  { value: "any", label: "Any" },
  { value: "needs_metadata", label: "Needs metadata" },
  { value: "partial", label: "Partial" },
  { value: "complete", label: "Complete" },
];

/**
 * Right-side sheet drawer holding the metadata-driven filter facets:
 * status / year range / my-rating slider / multi-selects for
 * publisher / language / age rating / genres / tags / credits /
 * characters / teams / locations. Extracted from
 * `LibraryGridView.tsx` in audit-remediation M7.3.
 */
export function FilterSheet({
  open,
  onOpenChange,
  mode,
  libraryId,
  status,
  onStatus,
  metadataCompleteness,
  onMetadataCompleteness,
  readStatus,
  onReadStatus,
  yearFrom,
  yearTo,
  onYearFrom,
  onYearTo,
  ratingRange,
  onRatingRange,
  publishers,
  onPublishers,
  languages,
  onLanguages,
  ageRatings,
  onAgeRatings,
  genres,
  onGenres,
  tags,
  onTags,
  credits,
  onCredit,
  characters,
  onCharacters,
  teams,
  onTeams,
  locations,
  onLocations,
  activeCount,
  onClear,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  mode: LibraryGridMode;
  libraryId: string | null;
  status: string;
  onStatus: (v: string) => void;
  metadataCompleteness: MetadataCompletenessTier | undefined;
  onMetadataCompleteness: (v: MetadataCompletenessTier | undefined) => void;
  readStatus: string[];
  onReadStatus: (v: string[]) => void;
  yearFrom: string;
  yearTo: string;
  onYearFrom: (v: string) => void;
  onYearTo: (v: string) => void;
  ratingRange: [number, number] | null;
  onRatingRange: (v: [number, number] | null) => void;
  publishers: string[];
  onPublishers: (v: string[]) => void;
  languages: string[];
  onLanguages: (v: string[]) => void;
  ageRatings: string[];
  onAgeRatings: (v: string[]) => void;
  genres: string[];
  onGenres: (v: string[]) => void;
  tags: string[];
  onTags: (v: string[]) => void;
  credits: CreditState;
  onCredit: (key: CreditKey, values: string[]) => void;
  characters: string[];
  onCharacters: (v: string[]) => void;
  teams: string[];
  onTeams: (v: string[]) => void;
  locations: string[];
  onLocations: (v: string[]) => void;
  activeCount: number;
  onClear: () => void;
}) {
  // Forward `library` to the options endpoints so per-library views
  // only surface values that exist in that library.
  const optsLibrary = libraryId ?? undefined;
  const ratingDraft: [number, number] = ratingRange ?? [RATING_MIN, RATING_MAX];
  // Re-anchor the descendant `MultiSelectEditor` popovers into the
  // SheetContent subtree. Without this they portal to document.body
  // and Radix's Sheet modal aria-hides them — items render but reject
  // focus/clicks. `overflow-visible` so a wide picker can extend past
  // the sheet edge when needed; the inner body div owns the scroll.
  const [portalContainer, setPortalContainer] =
    React.useState<HTMLElement | null>(null);
  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      <SheetContent
        ref={setPortalContainer}
        side="right"
        className="flex w-full flex-col gap-0 overflow-visible p-0 sm:max-w-md"
      >
        <SheetHeader className="border-border/60 flex-row items-center justify-between border-b pt-[max(1rem,var(--safe-top))] pr-[max(3rem,calc(var(--safe-right)+2rem))] pb-4 pl-6">
          <div>
            <SheetTitle>Filters</SheetTitle>
            <SheetDescription>
              {activeCount > 0
                ? `${activeCount} active`
                : "Narrow the library by metadata."}
            </SheetDescription>
          </div>
          {activeCount > 0 ? (
            <Button
              type="button"
              variant="ghost"
              size="sm"
              onClick={onClear}
              className="h-8"
            >
              Clear all
            </Button>
          ) : null}
        </SheetHeader>
        <PopoverPortalContainer value={portalContainer}>
          <div className="min-h-0 flex-1 overflow-y-auto">
            {/* Status is series-only (issues don't carry one) — hide
                the section when the grid is in issues mode rather
                than disabling, so the picker stays uncluttered. */}
            {mode === "series" ? (
              <Section title="Status" defaultOpen>
                <Select value={status} onValueChange={onStatus}>
                  <SelectTrigger>
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {STATUS_OPTIONS.map((o) => (
                      <SelectItem key={o.value} value={o.value}>
                        {o.label}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </Section>
            ) : null}
            {/* Metadata-completeness rollup — series only. `needs_metadata`
                is the "Unmatched" worklist; the cover "meta" badge surfaces
                the same tier. */}
            {mode === "series" ? (
              <Section title="Metadata">
                <Select
                  value={metadataCompleteness ?? "any"}
                  onValueChange={(v) =>
                    onMetadataCompleteness(
                      v === "any" ? undefined : (v as MetadataCompletenessTier),
                    )
                  }
                >
                  <SelectTrigger>
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {METADATA_COMPLETENESS_OPTIONS.map((o) => (
                      <SelectItem key={o.value} value={o.value}>
                        {o.label}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </Section>
            ) : null}
            {/* Read status is a per-series rollup — series mode only. */}
            {mode === "series" ? (
              <Section title="Read status">
                <div className="flex flex-wrap gap-2">
                  {READ_STATUS_OPTIONS.map((o) => {
                    const active = readStatus.includes(o.value);
                    return (
                      <FilterPill
                        key={o.value}
                        active={active}
                        onClick={() =>
                          onReadStatus(
                            active
                              ? readStatus.filter((v) => v !== o.value)
                              : [...readStatus, o.value],
                          )
                        }
                      >
                        {o.label}
                      </FilterPill>
                    );
                  })}
                </div>
              </Section>
            ) : null}
            <Section title="Year">
              <div className="flex items-center gap-2">
                <Input
                  type="number"
                  inputMode="numeric"
                  placeholder="From"
                  value={yearFrom}
                  onChange={(e) => onYearFrom(e.target.value)}
                />
                <span className="text-muted-foreground text-xs">—</span>
                <Input
                  type="number"
                  inputMode="numeric"
                  placeholder="To"
                  value={yearTo}
                  onChange={(e) => onYearTo(e.target.value)}
                />
              </div>
            </Section>
            <Section title="My rating">
              <div className="space-y-3">
                <div className="text-muted-foreground flex justify-between text-xs tabular-nums">
                  <span>{ratingDraft[0].toFixed(1)} ★</span>
                  <span>{ratingDraft[1].toFixed(1)} ★</span>
                </div>
                <Slider
                  min={RATING_MIN}
                  max={RATING_MAX}
                  step={RATING_STEP}
                  value={ratingDraft}
                  onValueChange={(v) => {
                    if (
                      v.length === 2 &&
                      v[0] !== undefined &&
                      v[1] !== undefined
                    ) {
                      onRatingRange([v[0], v[1]]);
                    }
                  }}
                />
                <p className="text-muted-foreground text-xs">
                  Series you haven&apos;t rated are excluded when this filter is
                  active.
                </p>
                {ratingRange ? (
                  <Button
                    type="button"
                    variant="ghost"
                    size="sm"
                    onClick={() => onRatingRange(null)}
                    className="h-7 px-2 text-xs"
                  >
                    Clear rating filter
                  </Button>
                ) : null}
              </div>
            </Section>
            <FacetMultiSection
              title="Publisher"
              value={publishers}
              onChange={onPublishers}
              endpoint={{ kind: "publishers" }}
              library={optsLibrary}
            />
            <FacetMultiSection
              title="Language"
              value={languages}
              onChange={onLanguages}
              endpoint={{ kind: "languages" }}
              library={optsLibrary}
            />
            <FacetMultiSection
              title="Age rating"
              value={ageRatings}
              onChange={onAgeRatings}
              endpoint={{ kind: "age_ratings" }}
              library={optsLibrary}
            />
            <FacetMultiSection
              title="Genres"
              value={genres}
              onChange={onGenres}
              endpoint={{ kind: "genres" }}
              library={optsLibrary}
            />
            <FacetMultiSection
              title="Tags"
              value={tags}
              onChange={onTags}
              endpoint={{ kind: "tags" }}
              library={optsLibrary}
            />
            <Section title="Credits">
              <div className="space-y-3">
                {CREDIT_ROLES.map((c) => (
                  <div key={c.key} className="space-y-1">
                    <Label className="text-xs font-medium">{c.label}</Label>
                    <MultiSelectEditor
                      value={credits[c.key]}
                      onChange={(v) => onCredit(c.key, v)}
                      endpoint={{ kind: "credits", role: c.role }}
                      library={optsLibrary}
                      placeholder={`Any ${c.label.toLowerCase()}`}
                    />
                  </div>
                ))}
              </div>
            </Section>
            <FacetMultiSection
              title="Characters"
              value={characters}
              onChange={onCharacters}
              endpoint={{ kind: "characters" }}
              library={optsLibrary}
            />
            <FacetMultiSection
              title="Teams"
              value={teams}
              onChange={onTeams}
              endpoint={{ kind: "teams" }}
              library={optsLibrary}
            />
            <FacetMultiSection
              title="Locations"
              value={locations}
              onChange={onLocations}
              endpoint={{ kind: "locations" }}
              library={optsLibrary}
            />
          </div>
        </PopoverPortalContainer>
      </SheetContent>
    </Sheet>
  );
}

function FacetMultiSection({
  title,
  value,
  onChange,
  endpoint,
  library,
}: {
  title: string;
  value: string[];
  onChange: (v: string[]) => void;
  endpoint: OptionsEndpoint;
  library?: string;
}) {
  return (
    <Section title={title} badge={value.length > 0 ? value.length : undefined}>
      <MultiSelectEditor
        value={value}
        onChange={onChange}
        endpoint={endpoint}
        library={library}
        placeholder={`Any ${title.toLowerCase()}`}
      />
    </Section>
  );
}

function Section({
  title,
  badge,
  defaultOpen,
  children,
}: {
  title: string;
  badge?: number;
  defaultOpen?: boolean;
  children: React.ReactNode;
}) {
  return (
    <Collapsible
      defaultOpen={defaultOpen}
      className="group border-border/60 border-b last:border-b-0"
    >
      <CollapsibleTrigger className="hover:bg-accent/40 flex w-full cursor-pointer items-center justify-between px-6 py-3 text-xs font-semibold tracking-wider uppercase select-none">
        <span className="flex items-center gap-2">
          {title}
          {badge && badge > 0 ? (
            <Badge
              variant="secondary"
              className="h-5 min-w-5 rounded-full px-1.5 text-[10px]"
            >
              {badge}
            </Badge>
          ) : null}
        </span>
        <ChevronDown className="text-muted-foreground h-4 w-4 transition-transform group-data-[state=open]:rotate-180" />
      </CollapsibleTrigger>
      <CollapsibleContent className="space-y-2 px-6 pb-4">
        {children}
      </CollapsibleContent>
    </Collapsible>
  );
}

/**
 * Re-exported so the chip-strip and any external caller can render
 * matching status labels without duplicating the option table.
 */
export const LIBRARY_GRID_STATUS_OPTIONS = STATUS_OPTIONS;
export const LIBRARY_GRID_READ_STATUS_OPTIONS = READ_STATUS_OPTIONS;
export const LIBRARY_GRID_METADATA_COMPLETENESS_OPTIONS =
  METADATA_COMPLETENESS_OPTIONS;
