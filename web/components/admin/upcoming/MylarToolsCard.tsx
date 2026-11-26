"use client";

/**
 * Mylar tools on `/admin/upcoming` (add-to-mylar 1.0, round 2). Shown
 * once a Mylar address and key are stored.
 *
 * - **Search now** asks Mylar to search its indexers for its whole
 *   wanted list.
 * - **Check wanted list** reads Mylar's wanted list live and compares it
 *   with the library: issues Mylar still wants that the library already
 *   holds (per series: Recheck files, Refresh; per issue: Un-want), and
 *   how many reading-list issues Mylar is not looking for.
 *
 * Each button is one request to Mylar; nothing here runs on its own.
 */

import { Loader2, Search, ShieldCheck } from "lucide-react";
import * as React from "react";

import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import {
  useMylarSearchNow,
  useMylarSeriesAction,
  useMylarUnwant,
} from "@/lib/api/mutations";
import { useMylarWantedCheck } from "@/lib/api/queries";
import type { MylarOwnedWantedSeries, MylarWantedCheck } from "@/lib/api/types";

/** "3 of 412 wanted issues are already in your library, across 2 series." */
export function wantedCheckSummary(check: MylarWantedCheck): string {
  if (check.wanted === 0) return "Mylar's wanted list is empty.";
  const wanted = `${check.wanted.toLocaleString()} wanted ${
    check.wanted === 1 ? "issue" : "issues"
  }`;
  if (check.owned_issues === 0) {
    return `None of Mylar's ${wanted} ${
      check.wanted === 1 ? "is" : "are"
    } in your library.`;
  }
  return `${check.owned_issues.toLocaleString()} of Mylar's ${wanted} ${
    check.owned_issues === 1 ? "is" : "are"
  } already in your library, across ${check.owned_series_total} series.`;
}

/** "35 of the 120 issues your reading lists are missing are not wanted in Mylar." */
export function listGapSummary(check: MylarWantedCheck): string | null {
  if (check.list_missing === 0) return null;
  if (check.list_missing_not_wanted === check.list_missing) {
    return `Mylar wants none of the ${check.list_missing.toLocaleString()} issues your reading lists are missing. Add them from each list's Resolution tab.`;
  }
  if (check.list_missing_not_wanted === 0) {
    return `Mylar wants all ${check.list_missing.toLocaleString()} issues your reading lists are missing.`;
  }
  return `${check.list_missing_not_wanted.toLocaleString()} of the ${check.list_missing.toLocaleString()} issues your reading lists are missing are not wanted in Mylar. Add them from each list's Resolution tab.`;
}

export function MylarToolsCard() {
  const search = useMylarSearchNow();
  const check = useMylarWantedCheck();
  const data = check.data;
  return (
    <Card className="lg:col-span-2">
      <CardHeader className="flex flex-row flex-wrap items-center justify-between gap-2 pb-2">
        <CardTitle className="text-sm font-medium">Mylar tools</CardTitle>
        <div className="flex flex-wrap gap-2">
          <Button
            type="button"
            size="sm"
            variant="outline"
            disabled={check.isFetching}
            onClick={() => check.refetch()}
          >
            {check.isFetching ? (
              <Loader2 className="size-4 animate-spin" aria-hidden />
            ) : (
              <ShieldCheck className="size-4" aria-hidden />
            )}
            Check wanted list
          </Button>
          <Button
            type="button"
            size="sm"
            variant="outline"
            disabled={search.isPending}
            onClick={() => search.mutate()}
          >
            {search.isPending ? (
              <Loader2 className="size-4 animate-spin" aria-hidden />
            ) : (
              <Search className="size-4" aria-hidden />
            )}
            Search now
          </Button>
        </div>
      </CardHeader>
      <CardContent className="space-y-3">
        <p className="text-muted-foreground text-sm">
          Check wanted list compares what Mylar is looking for with your
          library. Search now makes Mylar search for its whole wanted list
          instead of waiting for its next scheduled search.
        </p>
        {check.isError ? (
          <p role="alert" className="text-destructive text-sm">
            {check.error instanceof Error
              ? check.error.message
              : "The check failed."}
          </p>
        ) : null}
        {data ? (
          <div className="space-y-3" data-mylar-wanted-check>
            <p className="text-sm font-medium">{wantedCheckSummary(data)}</p>
            {listGapSummary(data) ? (
              <p className="text-muted-foreground text-sm">
                {listGapSummary(data)}
              </p>
            ) : null}
            {data.owned.length > 0 ? (
              <>
                <p className="text-muted-foreground text-xs">
                  Usually Mylar has not noticed the file: Recheck files makes it
                  look again. Un-want tells Mylar to stop looking for one issue.
                  {data.owned_series_total > data.owned.length
                    ? ` Showing the first ${data.owned.length} of ${data.owned_series_total} series.`
                    : ""}
                </p>
                <ul className="divide-border border-border divide-y rounded-md border">
                  {data.owned.map((s) => (
                    <OwnedSeriesRow key={s.cv_series_id} series={s} />
                  ))}
                </ul>
              </>
            ) : null}
          </div>
        ) : null}
      </CardContent>
    </Card>
  );
}

function OwnedSeriesRow({ series }: { series: MylarOwnedWantedSeries }) {
  const act = useMylarSeriesAction();
  const unwant = useMylarUnwant();
  const run = (action: "recheck" | "refresh") =>
    act.mutate({ cvSeriesId: series.cv_series_id, action });
  return (
    <li className="flex flex-wrap items-start justify-between gap-3 p-3">
      <div className="min-w-0 flex-1 basis-52">
        <div className="truncate text-sm font-medium" title={series.series}>
          {series.series || `ComicVine ${series.cv_series_id}`}
        </div>
        <ul className="mt-1 flex flex-wrap gap-1.5">
          {series.issues.map((issue) => (
            <li
              key={issue.cv_issue_id}
              className="border-border flex items-center gap-1 rounded-md border py-0.5 pr-0.5 pl-2 text-xs"
            >
              #{issue.issue_number}
              <Button
                type="button"
                size="sm"
                variant="ghost"
                className="h-6 px-1.5 text-xs"
                disabled={unwant.isPending}
                aria-label={`Un-want ${series.series} #${issue.issue_number}`}
                onClick={() => unwant.mutate({ cvIssueId: issue.cv_issue_id })}
              >
                Un-want
              </Button>
            </li>
          ))}
        </ul>
      </div>
      <div className="flex shrink-0 gap-2">
        <Button
          type="button"
          size="sm"
          variant="outline"
          disabled={act.isPending}
          onClick={() => run("recheck")}
        >
          Recheck files
        </Button>
        <Button
          type="button"
          size="sm"
          variant="ghost"
          disabled={act.isPending}
          onClick={() => run("refresh")}
        >
          Refresh
        </Button>
      </div>
    </li>
  );
}
