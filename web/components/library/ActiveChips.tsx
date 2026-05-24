"use client";

import { X } from "lucide-react";

import {
  CREDIT_ROLES,
  type CreditKey,
  type CreditState,
} from "@/components/library/library-grid-filters";
import { LIBRARY_GRID_STATUS_OPTIONS } from "@/components/library/FilterSheet";
import { Badge } from "@/components/ui/badge";

/**
 * Renders the strip of removable chips shown above the library grid
 * to summarise which filters are active. One chip per facet entry;
 * each chip's `onRemove` clears just that one value. Extracted from
 * `LibraryGridView.tsx` in audit-remediation M7.3.
 */
export function ActiveChips({
  status,
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
}) {
  return (
    <div className="mb-4 flex flex-wrap gap-1.5">
      {status !== "any" ? (
        <Chip
          label={`Status: ${labelFor(LIBRARY_GRID_STATUS_OPTIONS, status)}`}
          onRemove={onClearStatus}
        />
      ) : null}
      {yearFrom || yearTo ? (
        <Chip
          label={`Year: ${yearFrom || "…"}–${yearTo || "…"}`}
          onRemove={onClearYear}
        />
      ) : null}
      {ratingRange ? (
        <Chip
          label={`Rating: ${ratingRange[0].toFixed(1)}–${ratingRange[1].toFixed(1)} ★`}
          onRemove={onClearRating}
        />
      ) : null}
      {publishers.map((v) => (
        <Chip
          key={`pub-${v}`}
          label={`Publisher: ${v}`}
          onRemove={() => onRemovePublisher(v)}
        />
      ))}
      {languages.map((v) => (
        <Chip
          key={`lang-${v}`}
          label={`Language: ${v}`}
          onRemove={() => onRemoveLanguage(v)}
        />
      ))}
      {ageRatings.map((v) => (
        <Chip
          key={`age-${v}`}
          label={`Age: ${v}`}
          onRemove={() => onRemoveAgeRating(v)}
        />
      ))}
      {genres.map((v) => (
        <Chip
          key={`gen-${v}`}
          label={`Genre: ${v}`}
          onRemove={() => onRemoveGenre(v)}
        />
      ))}
      {tags.map((v) => (
        <Chip
          key={`tag-${v}`}
          label={`Tag: ${v}`}
          onRemove={() => onRemoveTag(v)}
        />
      ))}
      {CREDIT_ROLES.flatMap((c) =>
        credits[c.key].map((v) => (
          <Chip
            key={`${c.key}-${v}`}
            label={`${c.label.replace(/s$/, "")}: ${v}`}
            onRemove={() => onRemoveCredit(c.key, v)}
          />
        )),
      )}
      {anyCredits.map((v) => (
        <Chip
          key={`credits-${v}`}
          label={`Credits: ${v}`}
          onRemove={() => onRemoveAnyCredit(v)}
        />
      ))}
      {characters.map((v) => (
        <Chip
          key={`char-${v}`}
          label={`Character: ${v}`}
          onRemove={() => onRemoveCharacter(v)}
        />
      ))}
      {teams.map((v) => (
        <Chip
          key={`team-${v}`}
          label={`Team: ${v}`}
          onRemove={() => onRemoveTeam(v)}
        />
      ))}
      {locations.map((v) => (
        <Chip
          key={`loc-${v}`}
          label={`Location: ${v}`}
          onRemove={() => onRemoveLocation(v)}
        />
      ))}
    </div>
  );
}

function Chip({ label, onRemove }: { label: string; onRemove: () => void }) {
  return (
    <Badge variant="secondary" className="gap-1 pr-1">
      {label}
      <button
        type="button"
        onClick={onRemove}
        className="hover:bg-muted-foreground/20 rounded-sm"
        aria-label={`Remove ${label}`}
      >
        <X className="h-3 w-3" />
      </button>
    </Badge>
  );
}

function labelFor(
  options: { value: string; label: string }[],
  value: string,
): string {
  return options.find((o) => o.value === value)?.label ?? value;
}
