"use client";

/**
 * `/admin/upcoming` (upcoming-releases 1.0): the release calendar's
 * switches and state.
 *
 * - **Settings** — `upcoming.enabled` and `upcoming.weeks_ahead`, saved
 *   through the shared `PATCH /admin/settings` (both are live).
 * - **Release calendar** — what the Metron list pass last did, how many
 *   issues each week holds, and "Sync now".
 * - **Mylar** (`MylarCard`) — the switch, the ingest token and the last
 *   snapshot a Mylar host pushed.
 *
 * This page is view-only towards any outside system: "Sync now" asks
 * Metron for release dates, nothing else.
 */

import { AlertTriangle, Loader2, RefreshCw, XCircle } from "lucide-react";
import * as React from "react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Progress } from "@/components/ui/progress";
import { Switch } from "@/components/ui/switch";
import {
  useSyncUpcomingCalendar,
  useUpdateSettings,
} from "@/lib/api/mutations";
import { useAdminSettings, useAdminUpcomingStatus } from "@/lib/api/queries";
import type {
  UpcomingCalendarStatusView,
  UpcomingStatusView,
} from "@/lib/api/types";
import { formatRelativeDate } from "@/lib/format";
import { monthShort } from "@/lib/log/overview";

import { MylarCard } from "./MylarCard";
import { statusToneText } from "@/lib/ui/status-tone";

/** Bounds of `upcoming.weeks_ahead` (server: `upcoming::MAX_WEEKS_AHEAD`). */
export const WEEKS_AHEAD_DEFAULT = 8;
export const WEEKS_AHEAD_MAX = 12;

/** "9 Nov" from a `YYYY-MM-DD` date, the short form of `dayLabel`
 *  (`lib/log/overview.ts`): fixed wording, no locale or time-zone shift. */
export function shortDay(isoDate: string): string {
  const day = Number(isoDate.slice(8, 10));
  const month = monthShort(isoDate);
  return day && month ? `${day} ${month}` : isoDate;
}

export function UpcomingAdmin() {
  const status = useAdminUpcomingStatus();
  const settings = useAdminSettings();

  if (status.isLoading || settings.isLoading) {
    return (
      <div className="text-muted-foreground flex items-center gap-2 py-6 text-sm">
        <Loader2 className="size-4 animate-spin" /> Loading…
      </div>
    );
  }
  if (!status.data || !settings.data) {
    return (
      <p className="text-destructive text-sm" role="alert">
        Couldn&rsquo;t load the upcoming-releases status.
      </p>
    );
  }
  const byKey: Record<string, unknown> = {};
  for (const row of settings.data.values) byKey[row.key] = row.value;
  const enabled = byKey["upcoming.enabled"] === true;
  const rawWeeks = Number(byKey["upcoming.weeks_ahead"]);
  const weeksAhead =
    Number.isFinite(rawWeeks) && rawWeeks > 0 ? rawWeeks : WEEKS_AHEAD_DEFAULT;

  return (
    <div className="grid grid-cols-1 gap-4 lg:grid-cols-2">
      <UpcomingSettingsCard
        key={`${enabled ? 1 : 0}-${weeksAhead}`}
        enabled={enabled}
        weeksAhead={weeksAhead}
      />
      <CalendarStatusCard status={status.data} />
      <MylarCard mylar={status.data.mylar} upcomingEnabled={enabled} />
    </div>
  );
}

function UpcomingSettingsCard({
  enabled: initialEnabled,
  weeksAhead: initialWeeks,
}: {
  enabled: boolean;
  weeksAhead: number;
}) {
  const update = useUpdateSettings();
  const [enabled, setEnabled] = React.useState(initialEnabled);
  const [weeks, setWeeks] = React.useState(String(initialWeeks));
  const weeksNum = Number(weeks);
  const weeksValid =
    Number.isInteger(weeksNum) && weeksNum >= 1 && weeksNum <= WEEKS_AHEAD_MAX;
  const dirty = enabled !== initialEnabled || weeksNum !== initialWeeks;

  const onSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    const patch: Record<string, unknown> = {};
    if (enabled !== initialEnabled) patch["upcoming.enabled"] = enabled;
    if (weeksValid && weeksNum !== initialWeeks) {
      patch["upcoming.weeks_ahead"] = weeksNum;
    }
    if (Object.keys(patch).length === 0) return;
    try {
      await update.mutateAsync(patch);
    } catch {
      // useApiMutation already toasts on error.
    }
  };

  return (
    <Card>
      <CardHeader className="pb-2">
        <CardTitle className="text-sm font-medium">Settings</CardTitle>
      </CardHeader>
      <CardContent>
        <form onSubmit={onSubmit} className="space-y-4">
          <div className="flex items-start gap-3">
            <Switch
              id="upcoming-enabled"
              checked={enabled}
              onCheckedChange={setEnabled}
              className="mt-0.5"
            />
            <div className="grid gap-1">
              <Label htmlFor="upcoming-enabled" className="cursor-pointer">
                Show upcoming releases
              </Label>
              <p className="text-muted-foreground text-xs">
                Once a day Folio asks Metron which issues go on sale in the
                coming weeks. It needs Metron enabled with credentials on the
                Metadata page.
              </p>
            </div>
          </div>
          <div className="grid gap-1.5">
            <Label htmlFor="upcoming-weeks-ahead">Weeks ahead</Label>
            <Input
              id="upcoming-weeks-ahead"
              type="number"
              inputMode="numeric"
              min={1}
              max={WEEKS_AHEAD_MAX}
              value={weeks}
              onChange={(e) => setWeeks(e.target.value)}
              aria-invalid={!weeksValid}
              aria-describedby="upcoming-weeks-ahead-help"
              className="w-24"
            />
            <p
              id="upcoming-weeks-ahead-help"
              className="text-muted-foreground text-xs"
            >
              1 to {WEEKS_AHEAD_MAX}. Metron&rsquo;s listings are complete about
              six weeks out and thin after eight.
            </p>
          </div>
          <Button
            type="submit"
            disabled={!dirty || !weeksValid || update.isPending}
          >
            {update.isPending ? (
              <>
                <Loader2 className="mr-2 size-4 animate-spin" />
                Saving
              </>
            ) : (
              "Save"
            )}
          </Button>
        </form>
      </CardContent>
    </Card>
  );
}

function stateBadge(status: UpcomingStatusView) {
  if (!status.enabled) {
    return (
      <Badge variant="outline" className="text-[10px]">
        OFF
      </Badge>
    );
  }
  if (!status.metron_ready) {
    return (
      <Badge variant="secondary" className="text-[10px]">
        METRON NOT READY
      </Badge>
    );
  }
  return (
    <Badge variant="default" className="text-[10px]">
      ON
    </Badge>
  );
}

function CalendarStatusCard({ status }: { status: UpcomingStatusView }) {
  const sync = useSyncUpcomingCalendar();
  const cal = status.calendar;
  const canSync = status.enabled && status.metron_ready;
  return (
    <Card>
      <CardHeader className="flex flex-row flex-wrap items-center justify-between gap-2 pb-2">
        <CardTitle className="flex items-center gap-2 text-sm font-medium">
          Release calendar
          {stateBadge(status)}
        </CardTitle>
        <Button
          size="sm"
          variant="outline"
          className="shrink-0 whitespace-nowrap"
          onClick={() => sync.mutate(undefined)}
          disabled={!canSync || sync.isPending}
        >
          {sync.isPending ? (
            <Loader2 className="mr-1 size-3 animate-spin" />
          ) : (
            <RefreshCw className="mr-1 size-3" />
          )}
          Sync now
        </Button>
      </CardHeader>
      <CardContent className="space-y-3 text-sm">
        {status.enabled && !status.metron_ready ? (
          <p className={`flex items-start gap-1 ${statusToneText("warning")}`}>
            <AlertTriangle className="mt-0.5 size-4 shrink-0" />
            <span>
              Metron is off or has no credentials, so nothing can be listed. Set
              it up on the Metadata page.
            </span>
          </p>
        ) : null}
        <CalendarSummary cal={cal} />
        <WeekBars cal={cal} />
      </CardContent>
    </Card>
  );
}

function CalendarSummary({ cal }: { cal: UpcomingCalendarStatusView }) {
  const lastSync = formatRelativeDate(cal.last_finished_at);
  const retryAt = cal.next_attempt_at
    ? new Date(cal.next_attempt_at).toLocaleTimeString(undefined, {
        hour: "numeric",
        minute: "2-digit",
      })
    : null;
  return (
    <div className="space-y-1">
      <p>
        <span className="font-medium tabular-nums">
          {cal.issues.toLocaleString()}
        </span>{" "}
        {cal.issues === 1 ? "issue" : "issues"} across{" "}
        <span className="font-medium tabular-nums">
          {cal.series.toLocaleString()}
        </span>{" "}
        series, {shortDay(cal.window_start)} to {shortDay(cal.window_end)}.
      </p>
      <p className="text-muted-foreground text-xs">
        {lastSync ? `Last full sync ${lastSync}.` : "No full sync yet."}
        {cal.resumes_at_week
          ? ` A sync paused at the week of ${shortDay(cal.resumes_at_week)} and resumes by itself.`
          : ""}
      </p>
      {cal.series_pending_detail > 0 ? (
        <p className="text-muted-foreground text-xs">
          {cal.series_pending_detail.toLocaleString()} series still waiting for
          publisher details (about ten a minute, soonest releases first).
        </p>
      ) : null}
      {cal.last_error ? (
        <p className="text-destructive flex items-start gap-1 text-xs">
          <XCircle className="mt-0.5 size-3 shrink-0" />
          <span>
            Last sync failed
            {formatRelativeDate(cal.last_error_at)
              ? ` ${formatRelativeDate(cal.last_error_at)}`
              : ""}
            : {cal.last_error}
            {retryAt ? ` Next attempt after ${retryAt}.` : ""}
          </span>
        </p>
      ) : null}
    </div>
  );
}

/** One bar per week: how far ahead the listings actually reach. */
function WeekBars({ cal }: { cal: UpcomingCalendarStatusView }) {
  if (cal.weeks.length === 0) return null;
  const max = Math.max(...cal.weeks.map((w) => w.issues), 1);
  return (
    <ul className="space-y-1.5" aria-label="Issues per week">
      {cal.weeks.map((w) => (
        <li
          key={w.week_start}
          className="grid grid-cols-[4.5rem_1fr_2.5rem] items-center gap-2 text-xs"
        >
          <span className="text-muted-foreground">
            {shortDay(w.week_start)}
          </span>
          <Progress
            value={(w.issues / max) * 100}
            aria-label={`Week of ${shortDay(w.week_start)}: ${w.issues} issues`}
          />
          <span className="text-right tabular-nums">{w.issues}</span>
        </li>
      ))}
    </ul>
  );
}
