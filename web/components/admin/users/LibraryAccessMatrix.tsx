"use client";

import * as React from "react";
import { Library as LibraryIcon } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { NativeSelect } from "@/components/folio/native-select";
import { Label } from "@/components/ui/label";
import { Skeleton } from "@/components/ui/skeleton";
import { Switch } from "@/components/ui/switch";
import {
  AGE_RATING_GROUPS,
  AGE_RATING_GROUPS_NOTE,
  groupOf,
} from "@/lib/age-rating";
import { useLibraryAgeRatingsAll, useLibraryList } from "@/lib/api/queries";
import { useUpdateLibraryAccess } from "@/lib/api/mutations";
import type { AdminUserDetailView } from "@/lib/api/types";
import {
  buildAccessRequest,
  capEffect,
  capsDirty,
  describeCapEffect,
  hideUnratedDirty,
  isDirty,
  loadGrants,
  selectionDiff,
  setCap,
  setHideUnrated,
  toggleSelection,
} from "./library-access-logic";

/** "No cap" first, then every ladder rung under its group heading. */
const CAP_OPTIONS = [{ value: "", label: "No cap" }];
const CAP_GROUPS = AGE_RATING_GROUPS.map((g) => ({
  label: g.heading,
  options: g.ratings.map((r) => ({ value: r, label: `Up to ${r}` })),
}));

export function LibraryAccessMatrix({ user }: { user: AdminUserDetailView }) {
  const { data: libraries, isLoading } = useLibraryList();
  // Series per rating rung, per library: the "what this hides" line is
  // worked out from it for whatever is selected, with no request per change.
  const { data: ageRatings, isError: ageRatingsFailed } =
    useLibraryAgeRatingsAll();
  const histograms = React.useMemo(
    () => new Map((ageRatings ?? []).map((h) => [h.library_id, h])),
    [ageRatings],
  );
  const update = useUpdateLibraryAccess(user.id);
  const hideUnratedHelpId = React.useId();

  // Stored caps are read the way the server enforces them (another
  // spelling of a rung loads as that rung; a non-rung loads as no cap), so
  // the select, the counts and the request agree with what readers see.
  const loaded = React.useMemo(
    () => loadGrants(user.library_access),
    [user.library_access],
  );
  const original = loaded.selected;
  const originalCaps = loaded.caps;
  const originalHideUnrated = loaded.hideUnrated;
  // What the polite status region says: written only from the admin's own
  // change of a cap or switch, never on load or refetch.
  const [announcement, setAnnouncement] = React.useState("");
  const [selected, setSelected] = React.useState<Set<string>>(original);
  const [caps, setCaps] = React.useState<Map<string, string>>(originalCaps);
  const [hideUnrated, setHideUnratedState] =
    React.useState<Set<string>>(originalHideUnrated);
  // Reset local state when the upstream grants change (e.g. after a save
  // invalidates the query and we re-fetch). Comparing the prev value during
  // render avoids the cascading-render footgun of useEffect.
  const [prevOriginal, setPrevOriginal] = React.useState(original);
  if (original !== prevOriginal) {
    setPrevOriginal(original);
    setSelected(new Set(original));
    setCaps(new Map(originalCaps));
    setHideUnratedState(new Set(originalHideUnrated));
  }

  const dirty =
    isDirty(original, selected) ||
    capsDirty(originalCaps, caps, selected) ||
    hideUnratedDirty(originalHideUnrated, hideUnrated, selected);
  const diff = selectionDiff(original, selected);
  const isAdmin = user.role === "admin";

  /** Announce a row's new figure after the admin changed its cap or switch. */
  function announce(
    libraryId: string,
    name: string,
    cap: string | null,
    hides: boolean,
  ) {
    const histogram = histograms.get(libraryId);
    if (isAdmin || !histogram) return;
    setAnnouncement(
      `${name}: ${describeCapEffect(capEffect(histogram, cap, hides))}`,
    );
  }

  if (isLoading) return <Skeleton className="h-64 w-full" />;

  if (!libraries || libraries.length === 0) {
    return (
      <p className="text-muted-foreground text-sm">
        No libraries exist yet. Create one before granting access.
      </p>
    );
  }

  return (
    <div className="space-y-4">
      {isAdmin ? (
        <p className="border-border bg-muted/40 text-muted-foreground rounded-md border px-3 py-2 text-xs">
          Admins implicitly have access to every library. Per-library grants
          here are stored but have no effect while the role stays{" "}
          <code className="font-mono">admin</code>, so no series counts are
          shown.
        </p>
      ) : null}

      <p className="sr-only" role="status" aria-live="polite">
        {announcement}
      </p>

      <ul className="divide-border border-border bg-card divide-y rounded-md border">
        {libraries.map((lib) => {
          const checked = selected.has(lib.id);
          const cap = caps.get(lib.id) ?? null;
          const hides = hideUnrated.has(lib.id);
          const group = groupOf(cap);
          const histogram = histograms.get(lib.id);
          const switchId = `lib-hide-unrated-${lib.id}`;
          const unenforced = loaded.unenforcedCaps.get(lib.id);
          return (
            <li key={lib.id} className="hover:bg-muted/40 space-y-2 px-4 py-2">
              <div className="flex items-center gap-3">
                <Checkbox
                  id={`lib-access-${lib.id}`}
                  checked={checked}
                  onCheckedChange={() =>
                    setSelected((prev) => toggleSelection(prev, lib.id))
                  }
                />
                <label
                  htmlFor={`lib-access-${lib.id}`}
                  className="flex min-w-0 flex-1 cursor-pointer items-center gap-2 text-sm"
                >
                  <LibraryIcon className="text-muted-foreground size-3.5 shrink-0" />
                  <span className="font-medium">{lib.name}</span>
                  <span className="text-muted-foreground truncate font-mono text-xs">
                    {lib.root_path}
                  </span>
                </label>
              </div>
              <div className="flex flex-wrap items-center gap-x-4 gap-y-2 pl-7">
                <NativeSelect
                  size="sm"
                  aria-label={`Age-rating cap for ${lib.name}`}
                  options={CAP_OPTIONS}
                  groups={CAP_GROUPS}
                  value={cap ?? ""}
                  disabled={!checked}
                  onChange={(next) => {
                    setCaps((prev) => setCap(prev, lib.id, next || null));
                    announce(lib.id, lib.name, next || null, hides);
                  }}
                />
                <div className="flex items-center gap-2">
                  <Switch
                    id={switchId}
                    size="sm"
                    checked={hides}
                    disabled={!checked}
                    aria-describedby={hideUnratedHelpId}
                    onCheckedChange={(on) => {
                      setHideUnratedState((prev) =>
                        setHideUnrated(prev, lib.id, on),
                      );
                      announce(lib.id, lib.name, cap, on);
                    }}
                  />
                  <Label htmlFor={switchId} className="text-sm font-normal">
                    Hide unrated{" "}
                    <span className="sr-only">series in {lib.name}</span>
                  </Label>
                </div>
              </div>
              {checked ? (
                <div className="text-muted-foreground space-y-0.5 pl-7 text-xs">
                  {unenforced !== undefined && !cap ? (
                    <p data-testid={`cap-unenforced-${lib.id}`}>
                      The stored cap &ldquo;{unenforced}&rdquo; is not a rating
                      on the list, so it hides nothing; saving clears it.
                    </p>
                  ) : null}
                  {group ? (
                    <p>
                      <span className="text-foreground font-medium">
                        {group.heading}:
                      </span>{" "}
                      {group.description}
                    </p>
                  ) : null}
                  {isAdmin ? null : histogram ? (
                    <p data-testid={`cap-effect-${lib.id}`}>
                      {describeCapEffect(capEffect(histogram, cap, hides))}
                    </p>
                  ) : ageRatingsFailed ? (
                    <p data-testid={`cap-effect-${lib.id}`}>
                      Counts unavailable.
                    </p>
                  ) : (
                    // Loading: keep the line's room so it isn't read as
                    // "hides nothing" and the row doesn't jump.
                    <p aria-hidden="true" className="h-4" />
                  )}
                </div>
              ) : null}
            </li>
          );
        })}
      </ul>

      <div className="text-muted-foreground space-y-2 text-xs">
        <p>
          An age-rating cap hides series and issues rated above it (ComicInfo{" "}
          <code className="font-mono">AgeRating</code>). Most files carry no
          rating, and a cap alone leaves those visible.
        </p>
        <p id={hideUnratedHelpId}>
          <span className="text-foreground">Hide unrated</span> hides every
          series with no rating in that library, with or without a cap.
        </p>
        <p>
          The counts are series. An issue rated above the cap inside a series
          that stays visible is hidden too, and isn&apos;t counted.
        </p>
        <p>{AGE_RATING_GROUPS_NOTE}</p>
        <dl className="grid gap-x-3 gap-y-1 sm:grid-cols-[auto_1fr]">
          {AGE_RATING_GROUPS.map((g) => (
            <div key={g.heading} className="contents">
              <dt className="text-foreground font-medium">{g.heading}</dt>
              <dd>
                {g.description} <span>({g.ratings.join(", ")})</span>
              </dd>
            </div>
          ))}
        </dl>
      </div>

      <div className="flex items-center justify-between">
        <p className="text-muted-foreground text-xs">
          {dirty
            ? `${diff.added.length} to grant · ${diff.removed.length} to revoke`
            : "No pending changes"}
        </p>
        <div className="flex gap-2">
          <Button
            variant="ghost"
            size="sm"
            disabled={!dirty || update.isPending}
            onClick={() => {
              setSelected(new Set(original));
              setCaps(new Map(originalCaps));
              setHideUnratedState(new Set(originalHideUnrated));
            }}
          >
            Reset
          </Button>
          <Button
            size="sm"
            disabled={!dirty || update.isPending}
            onClick={() =>
              update.mutate(buildAccessRequest(selected, caps, hideUnrated))
            }
          >
            {update.isPending ? "Saving…" : "Save changes"}
          </Button>
        </div>
      </div>
    </div>
  );
}
