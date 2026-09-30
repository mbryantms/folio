"use client";

import { Loader2 } from "lucide-react";
import { useEffect, useState } from "react";

import { apiFetch } from "@/lib/api/auth-refresh";
import type { PageInfo } from "@/lib/api/types";
import {
  resolveOfflineResume,
  type DownloadManager,
  type OfflineResume,
  type ProgressLike,
} from "@/lib/pwa/downloads";
import {
  OFFLINE_SHELL_PATH,
  type DownloadRecord,
  type ReaderSnapshot,
} from "@/lib/pwa/offline-store";
import { getOutbox } from "@/lib/pwa/outbox";
import type { Direction, ViewMode } from "@/lib/reader/detect";
import { PROGRESS_OUTBOX_KIND } from "@/lib/reader/progress-writer";
import type { FitMode } from "@/lib/reader/store";

import { Reader } from "../read/[seriesSlug]/[issueSlug]/Reader";

const SERVER_PROGRESS_TIMEOUT_MS = 3000;

type Resolved = OfflineResume & { prefs: ReaderSnapshot };

const direction = (v: unknown): Direction | null =>
  v === "ltr" || v === "rtl" ? v : null;

/** Server progress when reachable; `null` offline or on any failure. */
async function serverProgress(issueId: string): Promise<ProgressLike | null> {
  if (typeof navigator !== "undefined" && !navigator.onLine) return null;
  try {
    const res = await Promise.race([
      apiFetch(`/progress?issue_id=${encodeURIComponent(issueId)}`, {
        headers: { Accept: "application/json" },
      }),
      new Promise<never>((_, reject) =>
        setTimeout(
          () => reject(new Error("timeout")),
          SERVER_PROGRESS_TIMEOUT_MS,
        ),
      ),
    ]);
    if (!res.ok) return null;
    const body = (await res.json()) as {
      records?: {
        issue_id: string;
        page: number;
        finished: boolean;
        run?: number;
      }[];
    };
    const mine = body.records?.find((r) => r.issue_id === issueId);
    return mine
      ? {
          issue_id: issueId,
          page: mine.page,
          finished: mine.finished,
          run: mine.run ?? 0,
        }
      : null;
  } catch {
    return null;
  }
}

/**
 * The regular reader, fed from IndexedDB instead of SSR (WP-4.6 offline
 * shell). Page and thumbnail requests from this document are answered by
 * the service worker from the downloaded copy; progress and reading
 * sessions go through the durable outbox exactly as online.
 */
export function OfflineReader({
  record,
  manager,
}: {
  record: DownloadRecord;
  manager: DownloadManager;
}) {
  const [resolved, setResolved] = useState<Resolved | null>(null);
  const issueId = record.issueId;

  useEffect(() => {
    let live = true;
    void (async () => {
      const [prefs, entries, server] = await Promise.all([
        manager.readerPrefs(),
        getOutbox()
          .entries<ProgressLike>({ kind: PROGRESS_OUTBOX_KIND })
          .catch(() => []),
        serverProgress(issueId),
      ]);
      const current = manager.get(issueId) ?? record;
      const queued = entries
        .map((e) => e.payload)
        .filter((p) => p.issue_id === issueId);
      const resume = resolveOfflineResume(current, queued, server);
      if (live) setResolved({ prefs, ...resume });
    })();
    return () => {
      live = false;
    };
    // Resolve once per opened issue; later record updates are our own
    // progress writes folding back in.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [issueId, manager]);

  if (!resolved) {
    return (
      <div
        className="bg-reader-bg grid min-h-screen place-items-center text-neutral-200"
        role="status"
      >
        <Loader2
          aria-hidden
          className="size-8 animate-spin text-neutral-600 motion-reduce:hidden"
        />
        <span className="sr-only">Opening downloaded issue…</span>
      </div>
    );
  }

  const { prefs } = resolved;
  const fit = prefs.default_fit_mode;
  const view = prefs.default_view_mode;
  const animation = prefs.default_page_animation;
  return (
    <Reader
      issueId={issueId}
      seriesId={record.seriesId}
      cblSavedViewId={null}
      exitUrl={OFFLINE_SHELL_PATH}
      totalPages={Math.max(1, record.pageCount || record.pages.length)}
      initialPage={resolved.initialPage}
      initialRun={resolved.initialRun}
      restartRun={resolved.restartRun}
      pages={record.pages as PageInfo[]}
      pageUrlVersion={record.contentVersion}
      manga={record.manga}
      userDefaultDirection={direction(prefs.default_reading_direction)}
      libraryDefaultDirection={direction(record.libraryDefaultReadingDirection)}
      seriesReadingDirection={direction(record.seriesReadingDirection)}
      userDefaultFitMode={
        fit === "width" || fit === "height" || fit === "original"
          ? (fit as FitMode)
          : null
      }
      userDefaultViewMode={
        view === "single" || view === "double" || view === "webtoon"
          ? (view as ViewMode)
          : null
      }
      userDefaultPageStrip={prefs.default_page_strip === true}
      userDefaultPageAnimation={
        animation === "off" || animation === "slide" || animation === "fade"
          ? animation
          : null
      }
      userDefaultCoverSolo={prefs.default_cover_solo !== false}
      userKeybinds={
        (prefs.keybinds as Record<string, string> | null | undefined) ?? {}
      }
      activityTrackingEnabled={prefs.activity_tracking_enabled !== false}
      readingMinActiveMs={prefs.reading_min_active_ms ?? 30_000}
      readingMinPages={prefs.reading_min_pages ?? 3}
      readingIdleMs={prefs.reading_idle_ms ?? 180_000}
    />
  );
}
