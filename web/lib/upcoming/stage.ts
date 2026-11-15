/**
 * Upcoming releases: what a release's stage means, in words and tone
 * (upcoming-releases 1.0). One place, so the card badge, the list row and
 * the detail dialog always say the same thing.
 *
 * Folio only reports these. Nothing here, or anywhere in the Upcoming
 * view, changes anything in Mylar.
 */
import { dayLabel, monthShort } from "@/lib/log/overview";
import type { StatusTone } from "@/lib/ui/status-tone";
import type { UpcomingEntryView, UpcomingStage } from "@/lib/api/types";

export const STAGE_LABEL: Record<UpcomingStage, string> = {
  in_library: "In library",
  downloaded: "Downloaded",
  snatched: "Downloading",
  wanted: "Wanted",
  tracked: "Tracked",
  announced: "Announced",
};

export const STAGE_TONE: Record<UpcomingStage, StatusTone> = {
  in_library: "success",
  downloaded: "success",
  snatched: "info",
  wanted: "warning",
  tracked: "info",
  announced: "neutral",
};

/** What the stage badge says. Mylar still counts a failed download as
 *  wanted; the reader should see that it failed. */
export function stageBadge(entry: UpcomingEntryView): {
  label: string;
  tone: StatusTone;
} {
  if (entry.stage === "wanted" && entry.mylar_status === "failed") {
    return { label: "Failed", tone: "error" };
  }
  return { label: STAGE_LABEL[entry.stage], tone: STAGE_TONE[entry.stage] };
}

/** One sentence for the detail dialog. */
export function stageSentence(entry: UpcomingEntryView): string {
  switch (entry.stage) {
    case "in_library":
      return "This issue is already in your Folio library.";
    case "downloaded":
      return "Mylar has downloaded it. It shows up in Folio after the next library scan.";
    case "snatched":
      return "Mylar has sent it to a download client.";
    case "wanted":
      return entry.mylar_status === "failed"
        ? "Mylar tried to get it and failed; it is still on Mylar's wanted list."
        : "Mylar is looking for it.";
    case "tracked":
      return "The series is on Mylar's watchlist. Mylar picks the issue up the week it comes out.";
    case "announced":
      return "Announced by the publisher. Nothing is tracking it.";
  }
}

/** Today in the viewer's own time zone, `YYYY-MM-DD`. The server only
 *  knows UTC, and "this week" turns over at the viewer's midnight. */
export function localToday(now: Date = new Date()): string {
  const y = now.getFullYear();
  const m = String(now.getMonth() + 1).padStart(2, "0");
  const d = String(now.getDate()).padStart(2, "0");
  return `${y}-${m}-${d}`;
}

function utc(date: string): number {
  return Date.parse(`${date}T00:00:00Z`);
}

/** Monday of the week containing `date` (weeks are Monday–Sunday). */
export function mondayOf(date: string): string {
  const d = new Date(utc(date));
  const back = (d.getUTCDay() + 6) % 7;
  d.setUTCDate(d.getUTCDate() - back);
  return d.toISOString().slice(0, 10);
}

/** "This week", "Next week", else "Week of Mon 7 Dec". */
export function weekHeading(weekStart: string, today: string): string {
  const weeks = Math.round((utc(weekStart) - utc(mondayOf(today))) / 6048e5);
  if (weeks === 0) return "This week";
  if (weeks === 1) return "Next week";
  return `Week of ${dayLabel(weekStart, today.slice(0, 4))}`;
}

/** "16–22 Nov", or "30 Nov – 6 Dec" across a month end. */
export function weekRange(weekStart: string, weekEnd: string): string {
  const from = +weekStart.slice(8, 10);
  const to = +weekEnd.slice(8, 10);
  return weekStart.slice(0, 7) === weekEnd.slice(0, 7)
    ? `${from}–${to} ${monthShort(weekEnd)}`
    : `${from} ${monthShort(weekStart)} – ${to} ${monthShort(weekEnd)}`;
}

/** "Wed 25": the day chip on a cover, inside a week that names the month. */
export function releaseDayShort(date: string): string {
  return dayLabel(date).split(" ").slice(0, 2).join(" ");
}

/** "Jan 2027", the date printed on the cover. */
export function coverMonth(date: string): string {
  return `${monthShort(date)} ${date.slice(0, 4)}`;
}

/** Where Folio learned of the release. */
export function sourceLabel(entry: UpcomingEntryView): string {
  switch (entry.source) {
    case "metron":
      return "Metron's release calendar";
    case "pull_list":
      return "Mylar's pull list";
    case "both":
      return "Metron's release calendar and Mylar's pull list";
  }
}

/** "Wed 25 Nov". */
export function releaseDay(date: string, today: string): string {
  return dayLabel(date, today.slice(0, 4));
}

/** "Action Comics #1104". */
export function entryTitle(entry: UpcomingEntryView): string {
  return `${entry.series_name} #${entry.number}`;
}
