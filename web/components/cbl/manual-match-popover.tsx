"use client";

import * as React from "react";
import { useQuery } from "@tanstack/react-query";
import { ArrowLeft, Loader2 } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import { apiFetch } from "@/lib/api/auth-refresh";
import { useLibraryList } from "@/lib/api/queries";
import { useClearMatchEntry, useManualMatchEntry } from "@/lib/api/mutations";
import type {
  CblEntryView,
  IssueListView,
  SeriesListView,
  SeriesView,
} from "@/lib/api/types";

/** Two-step manual-match picker:
 *  1. **Series** — disambiguates similarly-named series (different
 *     publication years / library / issue counts).
 *  2. **Issue** — once a series is locked in, browse or filter its
 *     issues and pick the one to wire to this CBL entry.
 *
 *  The flat issue search the popover used to do meant a user staring
 *  at "Star Wars #1" results couldn't tell which series each row
 *  belonged to. Branching the flow puts the disambiguation up front,
 *  then narrows the issue list to a single series.
 */
type Step =
  | { kind: "series"; query: string }
  | { kind: "issue"; series: SeriesView; query: string };

export function ManualMatchPopover({
  listId,
  entry,
  trigger,
  /** Default query for step 1; defaults to the entry's series_name so
   *  the picker lands on something useful before the user types. */
  initialQuery,
}: {
  listId: string;
  entry: CblEntryView;
  trigger: React.ReactNode;
  initialQuery?: string;
}) {
  const [open, setOpen] = React.useState(false);
  const seriesSeed = initialQuery ?? entry.series_name;
  const issueSeed = entry.issue_number;
  const [step, setStep] = React.useState<Step>(() => ({
    kind: "series",
    query: seriesSeed,
  }));

  // Reset to step 1 with the original seed every time the popover
  // closes — re-opening should always feel fresh, not pick up where
  // a previous session left off on a different entry.
  React.useEffect(() => {
    if (!open) {
      // eslint-disable-next-line react-hooks/set-state-in-effect
      setStep({ kind: "series", query: seriesSeed });
    }
  }, [open, seriesSeed]);

  const match = useManualMatchEntry(listId);
  const clear = useClearMatchEntry(listId);

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>{trigger}</PopoverTrigger>
      <PopoverContent className="w-[420px] p-0" align="start">
        {step.kind === "series" ? (
          <SeriesStep
            query={step.query}
            onQuery={(q) => setStep({ kind: "series", query: q })}
            onPick={(s) =>
              setStep({ kind: "issue", series: s, query: issueSeed })
            }
          />
        ) : (
          <IssueStep
            series={step.series}
            query={step.query}
            onQuery={(q) =>
              setStep({ kind: "issue", series: step.series, query: q })
            }
            onBack={() => setStep({ kind: "series", query: seriesSeed })}
            onPick={async (issueId) => {
              try {
                await match.mutateAsync({
                  entryId: entry.id,
                  req: { issue_id: issueId },
                });
                setOpen(false);
              } catch {
                // toast comes from useApiMutation
              }
            }}
            disabled={match.isPending}
          />
        )}
        {entry.match_status === "manual" ? (
          <div className="border-border flex justify-end border-t p-2">
            <Button
              type="button"
              size="sm"
              variant="ghost"
              disabled={clear.isPending}
              onClick={async () => {
                await clear.mutateAsync(entry.id);
                setOpen(false);
              }}
            >
              Clear match
            </Button>
          </div>
        ) : null}
      </PopoverContent>
    </Popover>
  );
}

function SeriesStep({
  query,
  onQuery,
  onPick,
}: {
  query: string;
  onQuery: (q: string) => void;
  onPick: (series: SeriesView) => void;
}) {
  const debounced = useDebouncedValue(query, 200);
  const search = useSeriesSearch(debounced);
  const libraries = useLibraryList();
  const libByid = React.useMemo(
    () => new Map((libraries.data ?? []).map((l) => [l.id, l.name])),
    [libraries.data],
  );

  const items = search.data?.items ?? [];
  const trimmed = debounced.trim();

  return (
    <>
      <div className="border-border border-b p-3">
        <Input
          autoFocus
          value={query}
          onChange={(e) => onQuery(e.target.value)}
          placeholder="Search series…"
        />
      </div>
      <div className="max-h-[320px] overflow-auto">
        {trimmed.length === 0 ? (
          <Hint>Type a series name to search.</Hint>
        ) : search.isFetching ? (
          <Spinner label="Searching…" />
        ) : items.length === 0 ? (
          <Hint>No series matched.</Hint>
        ) : (
          <ul className="divide-border divide-y">
            {items.map((s) => (
              <li key={s.id}>
                <button
                  type="button"
                  className="hover:bg-accent flex w-full flex-col gap-0.5 px-3 py-2 text-left text-sm"
                  onClick={() => onPick(s)}
                >
                  <div className="font-medium">
                    {s.name}
                    {s.year ? (
                      <span className="text-muted-foreground"> · {s.year}</span>
                    ) : null}
                  </div>
                  <div className="text-muted-foreground text-xs">
                    {issueCountLabel(s.issue_count)} ·{" "}
                    {libByid.get(s.library_id) ?? s.library_id}
                  </div>
                </button>
              </li>
            ))}
          </ul>
        )}
      </div>
    </>
  );
}

function IssueStep({
  series,
  query,
  onQuery,
  onBack,
  onPick,
  disabled,
}: {
  series: SeriesView;
  query: string;
  onQuery: (q: string) => void;
  onBack: () => void;
  onPick: (issueId: string) => void;
  disabled: boolean;
}) {
  const debounced = useDebouncedValue(query, 200);
  const issues = useSeriesIssuesPage(series.slug, debounced);
  const items = issues.data?.items ?? [];

  return (
    <>
      <div className="border-border flex items-center gap-2 border-b p-2">
        <Button
          type="button"
          size="sm"
          variant="ghost"
          onClick={onBack}
          className="px-2"
          aria-label="Back to series search"
        >
          <ArrowLeft className="h-3.5 w-3.5" />
        </Button>
        <div className="min-w-0 flex-1">
          <div className="truncate text-sm font-medium">{series.name}</div>
          <div className="text-muted-foreground truncate text-xs">
            {series.year ? `${series.year} · ` : ""}
            {issueCountLabel(series.issue_count)}
          </div>
        </div>
      </div>
      <div className="border-border border-b p-3">
        <Input
          autoFocus
          value={query}
          onChange={(e) => onQuery(e.target.value)}
          placeholder="Filter issues by number or title…"
        />
      </div>
      <div className="max-h-[320px] overflow-auto">
        {issues.isFetching && items.length === 0 ? (
          <Spinner label="Loading issues…" />
        ) : items.length === 0 ? (
          <Hint>No issues matched.</Hint>
        ) : (
          <ul className="divide-border divide-y">
            {items.map((iss) => (
              <li key={iss.id}>
                <button
                  type="button"
                  disabled={disabled}
                  className="hover:bg-accent flex w-full flex-col gap-0.5 px-3 py-2 text-left text-sm disabled:opacity-50"
                  onClick={() => onPick(iss.id)}
                >
                  <div className="font-medium">
                    {iss.number ? `#${iss.number}` : "—"}
                    {iss.title ? (
                      <span className="text-muted-foreground">
                        {" "}
                        {iss.title}
                      </span>
                    ) : null}
                  </div>
                  <div className="text-muted-foreground text-xs">
                    {iss.year ?? series.year ?? "—"}
                  </div>
                </button>
              </li>
            ))}
          </ul>
        )}
      </div>
    </>
  );
}

function useSeriesSearch(q: string) {
  const trimmed = q.trim();
  return useQuery({
    queryKey: ["series", "manual-match-search", trimmed],
    queryFn: async () => {
      const sp = new URLSearchParams({ limit: "20", q: trimmed });
      const res = await apiFetch(`/series?${sp.toString()}`);
      if (!res.ok) throw new Error(`series search ${res.status}`);
      return (await res.json()) as SeriesListView;
    },
    enabled: trimmed.length > 0,
    staleTime: 30_000,
  });
}

function useSeriesIssuesPage(seriesSlug: string, q: string) {
  const trimmed = q.trim();
  return useQuery({
    queryKey: ["series", "manual-match-issues", seriesSlug, trimmed],
    queryFn: async () => {
      const sp = new URLSearchParams({ limit: "60" });
      if (trimmed) sp.set("q", trimmed);
      else {
        sp.set("sort", "number");
        sp.set("order", "asc");
      }
      const res = await apiFetch(
        `/series/${seriesSlug}/issues?${sp.toString()}`,
      );
      if (!res.ok) throw new Error(`series issues ${res.status}`);
      return (await res.json()) as IssueListView;
    },
    enabled: !!seriesSlug,
    staleTime: 30_000,
  });
}

function useDebouncedValue<T>(value: T, ms: number): T {
  const [v, setV] = React.useState(value);
  React.useEffect(() => {
    const t = setTimeout(() => setV(value), ms);
    return () => clearTimeout(t);
  }, [value, ms]);
  return v;
}

function issueCountLabel(n: number | null | undefined): string {
  if (n == null || n === 0) return "no issues";
  return n === 1 ? "1 issue" : `${n} issues`;
}

function Spinner({ label }: { label: string }) {
  return (
    <div className="text-muted-foreground flex items-center gap-2 p-3 text-xs">
      <Loader2 className="h-3 w-3 animate-spin" /> {label}
    </div>
  );
}

function Hint({ children }: { children: React.ReactNode }) {
  return (
    <div className="text-muted-foreground p-3 text-center text-xs">
      {children}
    </div>
  );
}
