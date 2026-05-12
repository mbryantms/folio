import { notFound, redirect } from "next/navigation";
import { Reader } from "./Reader";
import { apiGet, ApiError } from "@/lib/api/fetch";
import type { IssueDetailView, MeView } from "@/lib/api/types";
import type { Direction, ViewMode } from "@/lib/reader/detect";
import type { FitMode } from "@/lib/reader/store";

type ProgressDelta = {
  records: Array<{
    issue_id: string;
    page: number;
    finished: boolean;
    updated_at: string;
  }>;
};

export default async function ReadPage({
  params,
  searchParams,
}: {
  params: Promise<{ seriesSlug: string; issueSlug: string }>;
  searchParams: Promise<{ from?: string; incognito?: string; page?: string }>;
}) {
  const { seriesSlug, issueSlug } = await params;
  const { from, incognito, page } = await searchParams;
  // `?from=start` is the "Read from beginning" entry point: skip the
  // saved-progress prefetch so the reader opens at page 0 even when the
  // user has prior progress on this issue. The reader's normal save-on-
  // page-change loop then catches up with the new position.
  const startFresh = from === "start";
  // `?incognito=1` disables both the reading-session tracker and the
  // per-page progress writes for this open. Saved progress is still
  // honored as a starting point unless `?from=start` is also set.
  const isIncognito = incognito === "1";
  // `?page=<n>` is the "Jump to page" deep-link used by /bookmarks. Like
  // `?from=start`, it overrides the saved-progress prefetch so the
  // bookmark's exact page wins — otherwise users with prior progress
  // land where they left off, not where the marker is.
  const explicitPage = parsePageParam(page);

  let issue: IssueDetailView;
  try {
    issue = await apiGet<IssueDetailView>(
      `/series/${seriesSlug}/issues/${issueSlug}`,
    );
  } catch (e) {
    if (e instanceof ApiError) {
      if (e.status === 401) {
        // SSR fetch has no session — bounce to sign-in instead of crashing
        // the route. Mirrors the (admin) and (settings) layout pattern.
        redirect(`/sign-in`);
      }
      if (e.status === 404) {
        notFound();
      }
    }
    throw e;
  }

  if (issue.state !== "active") {
    notFound();
  }

  // Best-effort progress prefetch — a 4xx just means "no record yet". Skip
  // entirely when the user explicitly asked to start fresh or jumped to a
  // specific page via `?page=`.
  let initialPage = 0;
  if (explicitPage !== null) {
    initialPage = explicitPage;
  } else if (!startFresh) {
    try {
      const delta = await apiGet<ProgressDelta>(`/progress`);
      const mine = delta.records.find((r) => r.issue_id === issue.id);
      if (mine) initialPage = mine.page;
    } catch {
      /* no progress yet */
    }
  }

  // page_count from ComicInfo isn't always trustworthy; if the reader walks
  // off the end we clamp client-side. 1 is the sane fallback so the reader
  // still mounts and the user gets an error if page 0 is also missing.
  const totalPages = Math.max(1, issue.page_count ?? 1);

  // Best-effort fetch of the user's reader prefs. Per-series localStorage and
  // the `Manga` flag still win over global defaults; these are only the
  // fallback for a fresh series with no other signal.
  let userDefaultDirection: Direction | null = null;
  let userDefaultFitMode: FitMode | null = null;
  let userDefaultViewMode: ViewMode | null = null;
  let userDefaultPageStrip = false;
  let userDefaultCoverSolo = true;
  let userKeybinds: Record<string, string> = {};
  let activityTrackingEnabled = true;
  let readingMinActiveMs = 30_000;
  let readingMinPages = 3;
  let readingIdleMs = 180_000;
  try {
    const me = await apiGet<MeView>("/auth/me");
    if (
      me.default_reading_direction === "ltr" ||
      me.default_reading_direction === "rtl"
    ) {
      userDefaultDirection = me.default_reading_direction;
    }
    if (
      me.default_fit_mode === "width" ||
      me.default_fit_mode === "height" ||
      me.default_fit_mode === "original"
    ) {
      userDefaultFitMode = me.default_fit_mode;
    }
    if (
      me.default_view_mode === "single" ||
      me.default_view_mode === "double" ||
      me.default_view_mode === "webtoon"
    ) {
      userDefaultViewMode = me.default_view_mode;
    }
    userDefaultPageStrip = me.default_page_strip === true;
    userDefaultCoverSolo = me.default_cover_solo !== false;
    userKeybinds = me.keybinds ?? {};
    activityTrackingEnabled = me.activity_tracking_enabled !== false;
    readingMinActiveMs = me.reading_min_active_ms ?? 30_000;
    readingMinPages = me.reading_min_pages ?? 3;
    readingIdleMs = me.reading_idle_ms ?? 180_000;
  } catch {
    /* unauthenticated or transient — fall back to ltr */
  }

  return (
    <Reader
      issueId={issue.id}
      seriesId={issue.series_id}
      exitUrl={`/series/${seriesSlug}/issues/${issueSlug}`}
      totalPages={totalPages}
      initialPage={initialPage}
      pages={issue.pages ?? []}
      manga={issue.manga ?? null}
      userDefaultDirection={userDefaultDirection}
      userDefaultFitMode={userDefaultFitMode}
      userDefaultViewMode={userDefaultViewMode}
      userDefaultPageStrip={userDefaultPageStrip}
      userDefaultCoverSolo={userDefaultCoverSolo}
      userKeybinds={userKeybinds}
      activityTrackingEnabled={activityTrackingEnabled && !isIncognito}
      incognito={isIncognito}
      readingMinActiveMs={readingMinActiveMs}
      readingMinPages={readingMinPages}
      readingIdleMs={readingIdleMs}
    />
  );
}

/** Parse the `?page=` query param into a non-negative integer. Returns
 *  `null` when the param is absent or malformed so the caller falls
 *  back to the normal saved-progress flow. The reader clamps against
 *  `totalPages` at mount, so we don't need an upper bound here. */
function parsePageParam(raw: string | undefined): number | null {
  if (raw === undefined || raw === "") return null;
  const n = Number.parseInt(raw, 10);
  if (!Number.isFinite(n) || n < 0) return null;
  return n;
}
