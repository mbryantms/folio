import { signInUrl } from "@/lib/api/sign-in-url";
import { notFound, redirect } from "next/navigation";
import { Reader } from "./Reader";
import { ReaderHealthToast } from "./ReaderHealthToast";
import { apiGet, ApiError } from "@/lib/api/fetch";
import type {
  IssueDetailView,
  MeView,
  PageInfo,
  PageOverridesView,
} from "@/lib/api/types";
import { detectInitialViewMode } from "@/lib/reader/detect";
import type { Direction, SeriesDirection, ViewMode } from "@/lib/reader/detect";
import type { FitMode } from "@/lib/reader/store";
import { preload } from "react-dom";
import { pageBytesSrcSet, withContentVersion } from "@/lib/urls";

// No `generateViewport` here on purpose: the reader inherits the root
// layout's viewport (theme-color / color-scheme) unchanged. It used to pin
// black/dark for light, amber and system themes, which rewrote those
// <meta> tags on every navigation into the reader — and an installed
// iPadOS app latches the first runtime change into a blurred status-bar
// strip over every page until the app is force-quit (seen on 26.1 and
// 27.0.1). See `lib/viewport.ts`.

type ProgressDelta = {
  records: Array<{
    issue_id: string;
    page: number;
    finished: boolean;
    /** Reading run (WP-1.3); absent on records written before the column existed. */
    run?: number;
    updated_at: string;
  }>;
};

export default async function ReadPage({
  params,
  searchParams,
}: {
  params: Promise<{ seriesSlug: string; issueSlug: string }>;
  searchParams: Promise<{
    from?: string;
    incognito?: string;
    page?: string;
    /** Saved-view id (kind=`cbl`) when the reader was entered from a CBL
     *  surface. Forwarded to the next-up resolver so "next" picks the
     *  next list entry rather than the next series issue. */
    cbl?: string;
    /** `?peek=1` puts the reader in peek mode — both progress writes
     *  and session tracking are suppressed until the user explicitly
     *  clicks "Continue from here" in the peek banner. Set by
     *  `buildJumpHref` so a bookmark click doesn't generate
     *  reading-activity noise when the user is just glancing at the
     *  marker's context. No timeout; the user controls when peek
     *  ends. */
    peek?: string;
  }>;
}) {
  const { seriesSlug, issueSlug } = await params;
  const { from, incognito, page, cbl, peek } = await searchParams;
  // Trust the URL contract: a malformed `?cbl=` just falls through to
  // series-next on the server. No client-side validation needed — the
  // resolver fails soft.
  const cblSavedViewId = typeof cbl === "string" && cbl.length > 0 ? cbl : null;
  // `?from=start` is the "Read from beginning" entry point: skip the
  // saved-progress prefetch so the reader opens at page 0 even when the
  // user has prior progress on this issue. The reader's normal save-on-
  // page-change loop then catches up with the new position.
  const startFresh = from === "start";
  // `?incognito=1` disables both the reading-session tracker and the
  // per-page progress writes for this open. Saved progress is still
  // honored as a starting point unless `?from=start` is also set.
  const isIncognito = incognito === "1";
  // `?peek=1` is like incognito but user-toggleable from inside the
  // reader (the peek banner's "Continue from here" button flips it
  // off and starts normal tracking). Used by bookmark "Jump to page"
  // navigation so glancing at a marker doesn't pollute the reading
  // log / On Deck / last-read state.
  const initialPeek = peek === "1";
  // `?page=<n>` is the "Jump to page" deep-link used by /bookmarks. Like
  // `?from=start`, it overrides the saved-progress prefetch so the
  // bookmark's exact page wins — otherwise users with prior progress
  // land where they left off, not where the marker is.
  const explicitPage = parsePageParam(page);

  // FEP-4: `/auth/me` is independent of the issue fetch, so kick it off now and
  // race it with the issue detail rather than awaiting after — shaves a
  // round-trip on the happy path (the window the loading skeleton is shown).
  // Fails soft to `null` (an unauthed/transient `/auth/me` → built-in defaults).
  const mePromise = apiGet<MeView>("/auth/me").catch(() => null);

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
        redirect(await signInUrl());
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

  // Progress is best-effort and fails soft to `null` (a 4xx just means "no
  // record yet"). Skipped entirely when the user asked to start fresh
  // (`?from=start`) or deep-linked a page (`?page=`). Awaited together with the
  // already-in-flight `mePromise` (kicked off before the issue fetch above).
  const progressPromise =
    explicitPage === null && !startFresh
      ? apiGet<ProgressDelta>(`/progress`).catch(() => null)
      : null;
  // WP-4.3: the user's manual spread controls, prefetched so the first
  // double-page paint already pairs with them. Fails soft to automatic.
  const overridesPromise = apiGet<PageOverridesView>(
    `/me/issues/${issue.id}/page-overrides`,
  ).catch(() => null);
  const [delta, me, pageOverrides] = await Promise.all([
    progressPromise,
    mePromise,
    overridesPromise,
  ]);

  // page_count from ComicInfo isn't always trustworthy; if the reader walks
  // off the end we clamp client-side. 1 is the sane fallback so the reader
  // still mounts and the user gets an error if page 0 is also missing.
  const totalPages = Math.max(1, issue.page_count ?? 1);

  let initialPage = 0;
  // Reading run of the saved record, and whether this open starts a new
  // one. "Read from beginning" always does; so does reopening a finished
  // issue from the cover — without a restart the server's within-run
  // floor (furthest page wins) would swallow the re-read's page turns.
  let initialRun = 0;
  let restartRun = startFresh;
  if (explicitPage !== null) {
    initialPage = explicitPage;
  } else if (delta) {
    const mine = delta.records.find((r) => r.issue_id === issue.id);
    // A finished issue parked on its last page is a dead-end open —
    // the next tap would just pop the end-of-issue card. Re-reads
    // start from the cover instead (Komga behavior). Mid-issue
    // finished records (bookmark jumps with sticky `finished`) still
    // resume where the reader left off.
    if (mine) {
      const parkedAtEnd = mine.finished && mine.page >= totalPages - 1;
      initialPage = parkedAtEnd ? 0 : mine.page;
      initialRun = mine.run ?? 0;
      if (parkedAtEnd) restartRun = true;
    }
  }

  // Series + library reading-direction layers of the resolution chain
  // (see `manga-and-bulk-metadata-1.0`). Both surfaced on
  // IssueDetailView so the read page doesn't need a second fetch. The
  // series layer also carries `ttb` ("Vertical (webtoon)"), which the
  // reader maps to webtoon view (WP-4.2).
  const seriesReadingDirection: SeriesDirection | null =
    issue.series_reading_direction === "ltr" ||
    issue.series_reading_direction === "rtl" ||
    issue.series_reading_direction === "ttb"
      ? issue.series_reading_direction
      : null;
  const libraryDefaultDirection: Direction | null =
    issue.library_default_reading_direction === "ltr" ||
    issue.library_default_reading_direction === "rtl"
      ? issue.library_default_reading_direction
      : null;

  // Best-effort fetch of the user's reader prefs. Per-series localStorage and
  // the `Manga` flag still win over global defaults; these are only the
  // fallback for a fresh series with no other signal.
  let userDefaultDirection: Direction | null = null;
  let userDefaultFitMode: FitMode | null = null;
  let userDefaultViewMode: ViewMode | null = null;
  let userDefaultPageStrip = false;
  let userDefaultPageAnimation: "off" | "slide" | "fade" | null = null;
  let userDefaultCoverSolo = true;
  let userKeybinds: Record<string, string> = {};
  let activityTrackingEnabled = true;
  let readingMinActiveMs = 30_000;
  let readingMinPages = 3;
  let readingIdleMs = 180_000;
  if (me) {
    if (
      me.default_reading_direction === "ltr" ||
      me.default_reading_direction === "rtl"
    ) {
      userDefaultDirection = me.default_reading_direction;
    }
    if (
      me.default_fit_mode === "width" ||
      me.default_fit_mode === "height" ||
      me.default_fit_mode === "original" ||
      me.default_fit_mode === "contain"
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
    if (
      me.default_page_animation === "off" ||
      me.default_page_animation === "slide" ||
      me.default_page_animation === "fade"
    ) {
      userDefaultPageAnimation = me.default_page_animation;
    }
    userDefaultCoverSolo = me.default_cover_solo !== false;
    userKeybinds =
      (me.keybinds as Record<string, string> | null | undefined) ?? {};
    activityTrackingEnabled = me.activity_tracking_enabled !== false;
    readingMinActiveMs = me.reading_min_active_ms ?? 30_000;
    readingMinPages = me.reading_min_pages ?? 3;
    readingIdleMs = me.reading_idle_ms ?? 180_000;
  }

  const pages = (issue.pages as PageInfo[] | null | undefined) ?? [];
  const firstPage = Math.min(initialPage, totalPages - 1);
  preloadFirstPage({
    issueId: issue.id,
    page: firstPage,
    pageInfo: pages[firstPage],
    version: issue.last_rewrite_at ?? null,
    viewMode: detectInitialViewMode(
      pages,
      userDefaultViewMode,
      seriesReadingDirection,
    ),
    fitMode: userDefaultFitMode ?? "width",
  });

  return (
    <>
      <ReaderHealthToast seriesSlug={seriesSlug} issueSlug={issueSlug} />
      <Reader
        issueId={issue.id}
        seriesId={issue.series_id}
        cblSavedViewId={cblSavedViewId}
        exitUrl={`/series/${seriesSlug}/issues/${issueSlug}`}
        totalPages={totalPages}
        initialPage={initialPage}
        initialRun={initialRun}
        restartRun={restartRun}
        pages={pages}
        pageUrlVersion={issue.last_rewrite_at ?? null}
        manga={issue.manga ?? null}
        userDefaultDirection={userDefaultDirection}
        libraryDefaultDirection={libraryDefaultDirection}
        seriesReadingDirection={seriesReadingDirection}
        userDefaultFitMode={userDefaultFitMode}
        userDefaultViewMode={userDefaultViewMode}
        userDefaultPageStrip={userDefaultPageStrip}
        userDefaultPageAnimation={userDefaultPageAnimation}
        userDefaultCoverSolo={userDefaultCoverSolo}
        userKeybinds={userKeybinds}
        activityTrackingEnabled={activityTrackingEnabled && !isIncognito}
        incognito={isIncognito}
        initialPeek={initialPeek}
        readingMinActiveMs={readingMinActiveMs}
        readingMinPages={readingMinPages}
        readingIdleMs={readingIdleMs}
        initialPageOverrides={pageOverrides}
      />
    </>
  );
}

/** Emit `<link rel="preload">` hints (React DOM `preload()`, hoisted into
 *  the document head) for the page the reader opens on and its blurred
 *  strip thumb, so the browser starts fetching both before the reader's
 *  JS has loaded and hydrated. The `<img>` itself also carries
 *  `fetchpriority="high"`. WP-4.4.
 *
 *  The hint mirrors the `srcSet` / `sizes` the page views render for the
 *  same defaults the Reader initialises its store from (single + webtoon:
 *  `100vw`, double: `50vw`; single-page original fit: full-res only), so
 *  the preload and the eventual `<img>` resolve to the same URL. A
 *  per-series localStorage override can still change the view after
 *  hydration, in which case the browser just drops the unused hint. */
function preloadFirstPage({
  issueId,
  page,
  pageInfo,
  version,
  viewMode,
  fitMode,
}: {
  issueId: string;
  page: number;
  pageInfo: PageInfo | undefined;
  version: string | null;
  viewMode: ViewMode;
  fitMode: FitMode;
}) {
  const src = withContentVersion(`/issues/${issueId}/pages/${page}`, version);
  const imageSrcSet =
    viewMode === "webtoon" || fitMode !== "original"
      ? pageBytesSrcSet(src, pageInfo?.image_width)
      : undefined;
  preload(src, {
    as: "image",
    fetchPriority: "high",
    ...(imageSrcSet
      ? { imageSrcSet, imageSizes: viewMode === "double" ? "50vw" : "100vw" }
      : {}),
  });
  preload(
    withContentVersion(
      `/issues/${issueId}/pages/${page}/thumb?variant=strip`,
      version,
    ),
    { as: "image" },
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
