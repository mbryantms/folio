"use client";

import * as React from "react";
import { X } from "lucide-react";

import {
  CREDIT_ROLES,
  type CreditKey,
  type CreditState,
  type MetadataCompletenessTier,
  PROVIDER_MATCH_LABELS,
  PROVIDER_SOURCE_LABELS,
  type ProviderSource,
  type ProviderMatch,
} from "@/components/library/library-grid-filters";
import {
  LIBRARY_GRID_METADATA_COMPLETENESS_OPTIONS,
  LIBRARY_GRID_READ_STATUS_OPTIONS,
  LIBRARY_GRID_STATUS_OPTIONS,
} from "@/components/library/FilterSheet";
import { Badge } from "@/components/ui/badge";
import { cn } from "@/lib/utils";
import { useReducedMotion } from "@/lib/visual-effects";

/**
 * Renders the strip of removable chips shown above the library grid
 * to summarise which filters are active. One chip per facet entry;
 * each chip's `onRemove` clears just that one value. Extracted from
 * `LibraryGridView.tsx` in audit-remediation M7.3.
 */
export function ActiveChips({
  className,
  status,
  metadataCompleteness,
  providerMatch,
  providerSource,
  readStatus,
  yearFrom,
  yearTo,
  ratingRange,
  publishers,
  languages,
  ageRatings,
  genres,
  tags,
  credits,
  anyCredits,
  characters,
  teams,
  locations,
  onClearStatus,
  onClearMetadataCompleteness,
  onClearProviderMatch,
  onRemoveReadStatus,
  onClearYear,
  onClearRating,
  onRemovePublisher,
  onRemoveLanguage,
  onRemoveAgeRating,
  onRemoveGenre,
  onRemoveTag,
  onRemoveCredit,
  onRemoveAnyCredit,
  onRemoveCharacter,
  onRemoveTeam,
  onRemoveLocation,
}: {
  status: string;
  metadataCompleteness: MetadataCompletenessTier | undefined;
  providerMatch: ProviderMatch | undefined;
  providerSource?: ProviderSource;
  readStatus: string[];
  yearFrom: string;
  yearTo: string;
  ratingRange: [number, number] | null;
  publishers: string[];
  languages: string[];
  ageRatings: string[];
  genres: string[];
  tags: string[];
  credits: CreditState;
  anyCredits: string[];
  characters: string[];
  teams: string[];
  locations: string[];
  onClearStatus: () => void;
  onClearMetadataCompleteness: () => void;
  onClearProviderMatch: () => void;
  onRemoveReadStatus: (v: string) => void;
  onClearYear: () => void;
  onClearRating: () => void;
  onRemovePublisher: (v: string) => void;
  onRemoveLanguage: (v: string) => void;
  onRemoveAgeRating: (v: string) => void;
  onRemoveGenre: (v: string) => void;
  onRemoveTag: (v: string) => void;
  onRemoveCredit: (role: CreditKey, v: string) => void;
  onRemoveAnyCredit: (v: string) => void;
  onRemoveCharacter: (v: string) => void;
  onRemoveTeam: (v: string) => void;
  onRemoveLocation: (v: string) => void;
  /** Extra classes on the row — the library grid insets it on phones
   *  in series mode so the last chip clears the A–Z rail. */
  className?: string;
}) {
  // Same order as the FilterSheet sections: reading, publication,
  // content, curation.
  const chips: ChipItem[] = [];
  const add = (key: string, label: string, onRemove: () => void) =>
    chips.push({ key, label, onRemove });
  for (const v of readStatus)
    add(
      `read-${v}`,
      `Read: ${labelFor(LIBRARY_GRID_READ_STATUS_OPTIONS, v)}`,
      () => onRemoveReadStatus(v),
    );
  if (ratingRange)
    add(
      "rating",
      `Rating: ${ratingRange[0].toFixed(1)}–${ratingRange[1].toFixed(1)} ★`,
      onClearRating,
    );
  if (status !== "any")
    add(
      "status",
      `Status: ${labelFor(LIBRARY_GRID_STATUS_OPTIONS, status)}`,
      onClearStatus,
    );
  if (yearFrom || yearTo)
    add("year", `Year: ${yearFrom || "…"}–${yearTo || "…"}`, onClearYear);
  for (const v of publishers)
    add(`pub-${v}`, `Publisher: ${v}`, () => onRemovePublisher(v));
  for (const v of ageRatings)
    add(`age-${v}`, `Age: ${v}`, () => onRemoveAgeRating(v));
  for (const v of languages)
    add(`lang-${v}`, `Language: ${v}`, () => onRemoveLanguage(v));
  for (const c of CREDIT_ROLES)
    for (const v of credits[c.key])
      add(`${c.key}-${v}`, `${c.label.replace(/s$/, "")}: ${v}`, () =>
        onRemoveCredit(c.key, v),
      );
  for (const v of anyCredits)
    add(`credits-${v}`, `Credits: ${v}`, () => onRemoveAnyCredit(v));
  for (const v of genres)
    add(`gen-${v}`, `Genre: ${v}`, () => onRemoveGenre(v));
  for (const v of tags) add(`tag-${v}`, `Tag: ${v}`, () => onRemoveTag(v));
  for (const v of characters)
    add(`char-${v}`, `Character: ${v}`, () => onRemoveCharacter(v));
  for (const v of teams) add(`team-${v}`, `Team: ${v}`, () => onRemoveTeam(v));
  for (const v of locations)
    add(`loc-${v}`, `Location: ${v}`, () => onRemoveLocation(v));
  if (metadataCompleteness)
    add(
      "meta",
      `Metadata: ${labelFor(
        LIBRARY_GRID_METADATA_COMPLETENESS_OPTIONS,
        metadataCompleteness,
      )}`,
      onClearMetadataCompleteness,
    );
  if (providerMatch)
    add(
      "provider",
      `Provider: ${PROVIDER_MATCH_LABELS[providerMatch]}${
        providerSource ? ` · ${PROVIDER_SOURCE_LABELS[providerSource]}` : ""
      }`,
      onClearProviderMatch,
    );

  const shown = useChipPresence(chips);
  if (shown.length === 0) return null;

  return (
    // Phone: one horizontally scrolling row (edge to edge inside the
    // shell's px-4) instead of a wrapping block that pushed the grid
    // down a row per three chips. sm+: wrap as before.
    <div
      className={cn(
        "mb-4 flex gap-1.5 max-sm:-mx-4 max-sm:[scrollbar-width:none] max-sm:flex-nowrap max-sm:overflow-x-auto max-sm:overflow-y-hidden max-sm:px-4 sm:flex-wrap max-sm:[&::-webkit-scrollbar]:hidden max-sm:[&>*]:shrink-0",
        className,
      )}
    >
      {shown.map((c) =>
        c.leaving ? (
          <LeavingChip key={c.key} label={c.label} />
        ) : (
          <Chip key={c.key} label={c.label} onRemove={c.onRemove} />
        ),
      )}
    </div>
  );
}

interface ChipItem {
  key: string;
  label: string;
  onRemove: () => void;
}

/** How long a removed chip stays mounted for its exit (matches
 *  `.filter-chip-leave` in styles/flair-loading.css). */
const CHIP_EXIT_MS = 240;

/**
 * Keeps removed chips mounted briefly, flagged `leaving`, at their old
 * position so they can fade/scale out (flair pre-merge polish). Under
 * reduced motion they just go. Works for every removal path (the chip's
 * own ×, the filter sheet, "clear all"), since it diffs the chip list.
 */
function useChipPresence(
  chips: ChipItem[],
): (ChipItem & { leaving?: boolean })[] {
  const reduceMotion = useReducedMotion();
  const signature = chips.map((c) => `${c.key}\u0000${c.label}`).join("\n");
  const [prev, setPrev] = React.useState({ signature, chips });
  const [leaving, setLeaving] = React.useState<
    { item: ChipItem; index: number }[]
  >([]);

  // Diff during render (the "adjust state on prop change" idiom): chips
  // that vanished since the last distinct list start leaving.
  if (prev.signature !== signature) {
    const now = new Set(chips.map((c) => c.key));
    const gone = reduceMotion
      ? []
      : prev.chips
          .map((item, index) => ({ item, index }))
          .filter(({ item }) => !now.has(item.key));
    setPrev({ signature, chips });
    setLeaving((l) => [...l.filter(({ item }) => !now.has(item.key)), ...gone]);
  }

  React.useEffect(() => {
    if (leaving.length === 0) return;
    const t = window.setTimeout(() => setLeaving([]), CHIP_EXIT_MS);
    return () => window.clearTimeout(t);
  }, [leaving]);

  const out: (ChipItem & { leaving?: boolean })[] = [...chips];
  for (const { item, index } of [...leaving].sort(
    (a, b) => a.index - b.index,
  )) {
    if (out.some((c) => c.key === item.key)) continue;
    out.splice(Math.min(index, out.length), 0, { ...item, leaving: true });
  }
  return out;
}

function Chip({ label, onRemove }: { label: string; onRemove: () => void }) {
  return (
    <Badge variant="secondary" className="filter-chip gap-1 pr-1">
      {label}
      <button
        type="button"
        onClick={onRemove}
        className="hover:bg-muted-foreground/20 rounded-sm"
        aria-label={`Remove ${label}`}
      >
        <X className="size-3" />
      </button>
    </Badge>
  );
}

/** A removed chip on its way out: inert, hidden from assistive tech. */
function LeavingChip({ label }: { label: string }) {
  return (
    <Badge
      variant="secondary"
      aria-hidden
      className="filter-chip filter-chip-leave gap-1 pr-1"
    >
      {label}
      <X className="size-3" />
    </Badge>
  );
}

function labelFor(
  options: { value: string; label: string }[],
  value: string,
): string {
  return options.find((o) => o.value === value)?.label ?? value;
}
