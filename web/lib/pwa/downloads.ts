/**
 * Per-issue offline downloads — the page-side download manager (WP-4.6,
 * offline plan steps 1, 3 and 4).
 *
 * Downloads an issue's pages at a chosen variant tier, plus its strip
 * thumbnails and cover, into Cache Storage (`folio-offline-v1:<account>`)
 * with its ordered metadata in IndexedDB (`lib/pwa/offline-store.ts`), so
 * the offline reader shell (`/downloads`) can open it with no network.
 *
 * - **Queue**: one issue at a time, `pageConcurrency` page fetches in
 *   flight (low fetch priority), so a download never competes with the
 *   reader. A series download enqueues each issue (walking every cursor
 *   page) and fetches each issue's detail when its turn comes.
 * - **Resumable**: already-stored pages are skipped, so pause → resume,
 *   a reload, or a dropped connection continue where they stopped; an
 *   interrupted download is re-queued on the next launch; `online`
 *   retries network failures.
 * - **Completeness**: only `complete` records are readable; a record is
 *   complete when every page is stored with image bytes. Strip thumbnails
 *   are best-effort (counted in `missingThumbs`). `verify` re-checks the
 *   cache and demotes a record whose pages were evicted.
 * - **Quota**: a `QuotaExceededError` stops the queue and marks the issue
 *   `error: "quota"` — nothing else is silently dropped. The first
 *   download asks for persistent storage (`navigator.storage.persist`).
 * - **Accounts**: records and caches belong to one account. A signed-in
 *   load (`setAccount`) purges every other account's downloads; explicit
 *   sign-out (`clearAll`) removes everything. Session expiry keeps them
 *   for offline reading (the same rule as the progress outbox).
 */

import type {
  IssueDetailView,
  IssueListView,
  IssueSummaryView,
  MeView,
  PageInfo,
} from "@/lib/api/types";
import { PROGRESS_OUTBOX_KIND } from "@/lib/reader/progress-writer";
import { pageBytesUrl, pageVariantUrl, withContentVersion } from "@/lib/urls";

import type { Outbox } from "./outbox";

import {
  DOWNLOAD_TIERS,
  META_ACCOUNT,
  META_READER,
  OFFLINE_CACHE_PREFIX,
  OFFLINE_SHELL_CACHE,
  indexedOfflineDb,
  offlineCacheName,
  offlineCoverKey,
  offlinePageKey,
  offlineStripKey,
  recordKey,
  type DownloadErrorKind,
  type DownloadRecord,
  type DownloadTier,
  type OfflineDb,
  type ReaderSnapshot,
} from "./offline-store";

export type DownloadEvent =
  | { type: "complete"; record: DownloadRecord }
  | { type: "error"; record: DownloadRecord };

export type StorageUsage = {
  usage: number | null;
  quota: number | null;
  persisted: boolean | null;
};

export interface DownloadManager {
  /** Scope to the signed-in account: purge every other account's
   *  downloads, then load and resume this one's. */
  setAccount(account: string): Promise<void>;
  /** Load the device owner's downloads without changing ownership (the
   *  offline shell, which may render without a session). */
  hydrate(): Promise<void>;
  account(): string | null;
  /** Sorted by creation; a new array on every change. */
  snapshot(): readonly DownloadRecord[];
  subscribe(listener: () => void): () => void;
  onEvent(listener: (event: DownloadEvent) => void): () => void;
  get(issueId: string): DownloadRecord | undefined;
  downloadIssue(
    issue: IssueDetailView,
    tier: DownloadTier,
    opts?: { reader?: ReaderSnapshot | null; seriesName?: string | null },
  ): Promise<void>;
  /** Enqueue every active issue of a series. Resolves with the number
   *  of issues queued (already-downloaded ones are skipped). */
  downloadSeries(
    series: { id: string; slug: string; name?: string | null },
    tier: DownloadTier,
    opts?: { reader?: ReaderSnapshot | null },
  ): Promise<number>;
  pause(issueId: string): Promise<void>;
  resume(issueId: string): Promise<void>;
  /** Remove one download (also cancels it when in flight). */
  remove(issueId: string): Promise<void>;
  /** Remove every download of the current account. */
  removeAll(): Promise<void>;
  /** Explicit sign-out: every download, every account, the shell. */
  clearAll(): Promise<void>;
  /** Re-check the cache; demote a complete record whose pages are gone.
   *  Resolves true when the download is complete and intact. */
  verify(issueId: string): Promise<boolean>;
  usage(): Promise<StorageUsage>;
  /** Reader preferences captured at download time (the offline reader
   *  has no `/auth/me`). */
  readerPrefs(): Promise<ReaderSnapshot>;
  /** Fold a progress write (live or queued) into the downloaded issue's
   *  offline resume position. No-op for issues that are not downloaded. */
  noteProgress(body: ProgressLike): Promise<void>;
}

/** The subset of a progress write the offline resume position needs. */
export type ProgressLike = {
  issue_id: string;
  page: number;
  finished?: boolean;
  run?: number;
  restart?: boolean;
};

export type OfflineProgress = NonNullable<DownloadRecord["progress"]>;

/**
 * Fold a progress write into a resume position with the server's run
 * rules (WP-1.3): a newer run replaces, the same run keeps the furthest
 * page and a sticky `finished`, an older run is ignored. A restart write
 * lands in the run after the one it carries.
 */
export function foldProgress(
  base: OfflineProgress | null,
  body: ProgressLike,
): OfflineProgress {
  const current = base ?? { page: 0, finished: false, run: 0 };
  const run =
    typeof body.run === "number"
      ? body.restart
        ? body.run + 1
        : body.run
      : current.run;
  if (run < current.run) return current;
  if (run > current.run)
    return { page: body.page, finished: body.finished === true, run };
  return {
    page: Math.max(current.page, body.page),
    finished: current.finished || body.finished === true,
    run,
  };
}

export type DownloadManagerDeps = {
  db?: OfflineDb;
  caches?: CacheStorage;
  /** Page/thumbnail byte fetches (credentialed, bare routes). */
  fetch?: typeof fetch;
  /** JSON API fetch — path without the `/api` prefix (`apiFetch`). */
  api?: (path: string) => Promise<Response>;
  now?: () => number;
  pageConcurrency?: number;
  storage?: StorageManager;
  /** Called after an issue completes (warms the offline shell). */
  afterComplete?: () => Promise<void> | void;
  online?: () => boolean;
};

const STRIP_THUMB_BYTES = 15_000;
const UNKNOWN_ORIGINAL_PAGE = 1_200_000;
/** Rough WebP q80 density for comic art at the variant tiers. */
const WEBP_BYTES_PER_PX = 0.2;
const SERIES_PAGE_SIZE = 100;

/** Estimated stored size of one issue at a tier (pages + strip thumbs). */
export function estimateIssueBytes(
  pages: PageInfo[],
  tier: DownloadTier,
  fallback: { pageCount?: number | null; fileSize?: number | null } = {},
): number {
  const count = pages.length || Math.max(0, fallback.pageCount ?? 0);
  const perOriginal =
    fallback.fileSize && count
      ? fallback.fileSize / count
      : UNKNOWN_ORIGINAL_PAGE;
  let total = 0;
  for (let i = 0; i < count; i++) {
    const p = pages[i];
    const original = p?.image_size ?? perOriginal;
    const w = p?.image_width ?? null;
    const h = p?.image_height ?? null;
    if (tier === "original" || (w != null && tier >= w)) {
      total += original;
    } else {
      const height = w && h ? (tier * h) / w : tier * 1.54;
      total += Math.min(original, tier * height * WEBP_BYTES_PER_PX);
    }
    total += STRIP_THUMB_BYTES;
  }
  return Math.round(total);
}

/** The tier a fit-width portrait read would pick on this device. */
export function defaultDownloadTier(
  screen?: { width: number; height: number },
  dpr = 1,
): DownloadTier {
  if (!screen) return 1080;
  const target = Math.min(screen.width, screen.height) * dpr;
  for (const t of DOWNLOAD_TIERS) {
    if (t !== "original" && t >= target) return t;
  }
  return "original";
}

/** Page-bytes URL for a download tier (never upscales past intrinsic). */
export function downloadPageUrl(
  issueId: string,
  page: number,
  tier: DownloadTier,
  width: number | null | undefined,
  version: string | null,
): string {
  let url = pageBytesUrl(issueId, page);
  if (tier !== "original" && (width == null || tier < width))
    url = pageVariantUrl(url, tier);
  return withContentVersion(url, version);
}

export function readerSnapshot(me: MeView | null | undefined): ReaderSnapshot {
  if (!me) return {};
  return {
    default_reading_direction: me.default_reading_direction ?? null,
    default_fit_mode: me.default_fit_mode ?? null,
    default_view_mode: me.default_view_mode ?? null,
    default_page_strip: me.default_page_strip ?? null,
    default_page_animation: me.default_page_animation ?? null,
    default_cover_solo: me.default_cover_solo ?? null,
    keybinds: me.keybinds ?? null,
    activity_tracking_enabled: me.activity_tracking_enabled ?? null,
    reading_min_active_ms: me.reading_min_active_ms ?? null,
    reading_min_pages: me.reading_min_pages ?? null,
    reading_idle_ms: me.reading_idle_ms ?? null,
  };
}

class HttpStatusError extends Error {
  constructor(readonly status: number) {
    super(`HTTP ${status}`);
  }
}

function isQuotaError(e: unknown): boolean {
  return (
    e instanceof DOMException &&
    (e.name === "QuotaExceededError" || e.code === 22)
  );
}

function isAbort(e: unknown): boolean {
  return e instanceof DOMException && e.name === "AbortError";
}

function classify(e: unknown): { kind: DownloadErrorKind; message: string } {
  if (isQuotaError(e))
    return {
      kind: "quota",
      message: "Device storage is full. Remove downloads to free space.",
    };
  if (e instanceof HttpStatusError) {
    if (e.status === 404 || e.status === 410)
      return { kind: "gone", message: "This issue is no longer available." };
    if (e.status === 401 || e.status === 403)
      return {
        kind: "network",
        message: "Sign in again to continue downloading.",
      };
    return { kind: "network", message: `The server answered ${e.status}.` };
  }
  if (e instanceof TypeError)
    return { kind: "network", message: "Connection lost. Retrying online." };
  return {
    kind: "unknown",
    message: e instanceof Error ? e.message : "Download failed.",
  };
}

async function mapLimit<T>(
  items: T[],
  limit: number,
  fn: (item: T) => Promise<void>,
): Promise<void> {
  let next = 0;
  let failure: unknown;
  const worker = async () => {
    while (failure === undefined && next < items.length) {
      const item = items[next++]!;
      try {
        await fn(item);
      } catch (e) {
        failure ??= e;
      }
    }
  };
  await Promise.all(
    Array.from({ length: Math.min(limit, items.length) }, worker),
  );
  if (failure !== undefined) throw failure;
}

function storedSize(response: Response): number {
  const n = Number(response.headers.get("content-length"));
  return Number.isFinite(n) ? n : 0;
}

export function createDownloadManager(
  deps: DownloadManagerDeps = {},
): DownloadManager {
  const db = deps.db ?? indexedOfflineDb();
  const cacheStorage = () => deps.caches ?? globalThis.caches;
  const doFetch = (input: string, init?: RequestInit) =>
    (deps.fetch ?? globalThis.fetch)(input, init);
  const api =
    deps.api ??
    (async (path: string) => {
      const { apiFetch } = await import("@/lib/api/auth-refresh");
      return apiFetch(path, { headers: { Accept: "application/json" } });
    });
  const now = deps.now ?? Date.now;
  const pageConcurrency = deps.pageConcurrency ?? 3;
  const isOnline = deps.online ?? (() => globalThis.navigator?.onLine ?? true);
  const storage = () => deps.storage ?? globalThis.navigator?.storage;

  let owner: string | null = null;
  const records = new Map<string, DownloadRecord>();
  let snap: readonly DownloadRecord[] = [];
  const listeners = new Set<() => void>();
  const eventListeners = new Set<(event: DownloadEvent) => void>();
  let active: { issueId: string; controller: AbortController } | null = null;
  let pumping = false;
  /** Set by a quota failure: the queue stays stopped until the user acts. */
  let halted = false;
  let persistAsked = false;
  let loaded: Promise<void> | null = null;

  const emit = () => {
    snap = [...records.values()].sort((a, b) => a.createdAt - b.createdAt);
    for (const l of listeners) l();
  };
  const fire = (event: DownloadEvent) => {
    for (const l of eventListeners) l(event);
  };

  const write = async (
    issueId: string,
    patch:
      | Partial<DownloadRecord>
      | ((r: DownloadRecord) => Partial<DownloadRecord>),
  ): Promise<DownloadRecord | null> => {
    if (!owner) return null;
    const key = recordKey(owner, issueId);
    const next = await db.update(key, (cur) => {
      if (!cur) return null;
      const delta = typeof patch === "function" ? patch(cur) : patch;
      return { ...cur, ...delta, updatedAt: now() };
    });
    if (next) records.set(issueId, next);
    else records.delete(issueId);
    emit();
    return next;
  };

  const loadOwner = async () => {
    records.clear();
    if (owner) {
      for (const r of await db.all()) {
        if (r.account !== owner) continue;
        // An in-flight download at the last unload is resumable.
        const row =
          r.status === "downloading"
            ? await db.update(r.key, (cur) =>
                cur ? { ...cur, status: "queued" } : null,
              )
            : r;
        if (row) records.set(row.issueId, row);
      }
    }
    emit();
  };

  const purgeOthers = async (account: string) => {
    for (const r of await db.all())
      if (r.account !== account) await db.delete(r.key);
    const cs = cacheStorage();
    if (!cs) return;
    for (const name of await cs.keys())
      if (
        name.startsWith(OFFLINE_CACHE_PREFIX) &&
        name !== offlineCacheName(account)
      )
        await cs.delete(name);
  };

  const cache = async () => {
    const cs = cacheStorage();
    if (!cs || !owner) throw new Error("Offline storage is unavailable.");
    return cs.open(offlineCacheName(owner));
  };

  const requestPersistence = async () => {
    if (persistAsked) return;
    persistAsked = true;
    try {
      const s = storage();
      if (s?.persisted && !(await s.persisted())) await s.persist?.();
    } catch {
      /* Denied or unsupported: the settings page says so. */
    }
  };

  const fetchJson = async <T>(path: string): Promise<T> => {
    const res = await api(path);
    if (!res.ok) throw new HttpStatusError(res.status);
    return (await res.json()) as T;
  };

  const storeAsset = async (
    target: Cache,
    key: string,
    url: string,
    signal: AbortSignal,
    required: boolean,
  ): Promise<number> => {
    const existing = await target.match(key);
    if (existing) return storedSize(existing);
    const res = await doFetch(url, {
      credentials: "include",
      signal,
      priority: "low",
    } as RequestInit);
    if (!res.ok) {
      if (required) throw new HttpStatusError(res.status);
      return 0;
    }
    const type = res.headers.get("content-type") ?? "";
    const blob = await res.blob();
    if (!type.startsWith("image/") || blob.size === 0) {
      if (required) throw new Error("The server returned no image bytes.");
      return 0;
    }
    if (signal.aborted) throw new DOMException("Aborted", "AbortError");
    await target.put(
      key,
      new Response(blob, {
        headers: {
          "content-type": type,
          "content-length": String(blob.size),
        },
      }),
    );
    return blob.size;
  };

  const run = async (issueId: string, controller: AbortController) => {
    const signal = controller.signal;
    let record = records.get(issueId);
    if (!record) return;
    record = (await write(issueId, {
      status: "downloading",
      error: null,
      errorMessage: null,
    }))!;
    if (!record) return;
    if (record.pages.length === 0) {
      const issue = await fetchJson<IssueDetailView>(
        `/series/${encodeURIComponent(record.seriesSlug)}/issues/${encodeURIComponent(record.issueSlug)}`,
      );
      if (issue.state !== "active") throw new HttpStatusError(404);
      const pages = (issue.pages as PageInfo[] | null | undefined) ?? [];
      record = (await write(issueId, {
        ...detailFields(issue),
        pages,
        pageCount: Math.max(pages.length, issue.page_count ?? 0),
        estimatedBytes: estimateIssueBytes(pages, record.tier, {
          pageCount: issue.page_count,
          fileSize: issue.file_size,
        }),
      }))!;
      if (!record) return;
    }
    if (!record.progress) {
      const progress = await fetchJson<{
        records: {
          issue_id: string;
          page: number;
          finished: boolean;
          run?: number;
        }[];
      }>(`/progress?issue_id=${encodeURIComponent(issueId)}`).catch(() => null);
      const mine = progress?.records.find((p) => p.issue_id === issueId);
      if (mine)
        record = (await write(issueId, {
          progress: {
            page: mine.page,
            finished: mine.finished,
            run: mine.run ?? 0,
          },
        }))!;
      if (!record) return;
    }
    const target = await cache();
    const total = Math.max(record.pages.length, record.pageCount);
    const indices = Array.from({ length: total }, (_, i) => i);
    let bytes = 0;
    let done = 0;
    const tier = record.tier;
    const version = record.contentVersion;
    const pages = record.pages;
    await mapLimit(indices, pageConcurrency, async (i) => {
      if (signal.aborted) throw new DOMException("Aborted", "AbortError");
      bytes += await storeAsset(
        target,
        offlinePageKey(issueId, i),
        downloadPageUrl(issueId, i, tier, pages[i]?.image_width, version),
        signal,
        true,
      );
      done++;
      await write(issueId, { donePages: done, bytes });
    });
    let missingThumbs = 0;
    await mapLimit(indices, pageConcurrency, async (i) => {
      if (signal.aborted) throw new DOMException("Aborted", "AbortError");
      const stored = await storeAsset(
        target,
        offlineStripKey(issueId, i),
        withContentVersion(
          `/issues/${encodeURIComponent(issueId)}/pages/${i}/thumb?variant=strip`,
          version,
        ),
        signal,
        false,
      ).catch((e) => {
        if (isQuotaError(e) || isAbort(e)) throw e;
        return 0;
      });
      if (stored === 0) missingThumbs++;
      bytes += stored;
    });
    bytes += await storeAsset(
      target,
      offlineCoverKey(issueId),
      `/issues/${encodeURIComponent(issueId)}/pages/0/thumb`,
      signal,
      false,
    ).catch((e) => {
      if (isQuotaError(e) || isAbort(e)) throw e;
      return 0;
    });
    const finished = await write(issueId, {
      status: "complete",
      donePages: total,
      bytes,
      missingThumbs,
      completedAt: now(),
    });
    if (finished) {
      fire({ type: "complete", record: finished });
      try {
        await deps.afterComplete?.();
      } catch {
        /* Shell warming is best-effort; the list still works online. */
      }
    }
  };

  const pump = async () => {
    if (pumping || halted || !owner) return;
    pumping = true;
    try {
      while (!halted && isOnline()) {
        const next = snap.find((r) => r.status === "queued");
        if (!next) break;
        const controller = new AbortController();
        active = { issueId: next.issueId, controller };
        try {
          await run(next.issueId, controller);
        } catch (e) {
          if (isAbort(e) || controller.signal.aborted) {
            // pause()/remove() already recorded the new state.
          } else {
            const { kind, message } = classify(e);
            if (kind === "quota") halted = true;
            const failed = await write(next.issueId, {
              status: "error",
              error: kind,
              errorMessage: message,
            });
            if (failed) fire({ type: "error", record: failed });
            // A dropped connection stops the queue until `online`.
            if (kind === "network") break;
          }
        } finally {
          active = null;
        }
      }
    } finally {
      pumping = false;
    }
  };

  const onOnline = () => {
    void (async () => {
      for (const r of snap)
        if (r.status === "error" && r.error === "network")
          await write(r.issueId, {
            status: "queued",
            error: null,
            errorMessage: null,
          });
      void pump();
    })();
  };
  if (typeof window !== "undefined")
    window.addEventListener("online", onOnline);

  const ensureOwner = () => {
    if (!owner)
      throw new Error("Sign in to download issues for offline reading.");
    return owner;
  };

  const baseRecord = (
    account: string,
    fields: Pick<
      DownloadRecord,
      "issueId" | "seriesId" | "seriesSlug" | "issueSlug"
    > &
      Partial<DownloadRecord>,
    tier: DownloadTier,
  ): DownloadRecord => ({
    key: recordKey(account, fields.issueId),
    account,
    seriesName: null,
    title: null,
    number: null,
    tier,
    status: "queued",
    error: null,
    errorMessage: null,
    pages: [],
    pageCount: 0,
    donePages: 0,
    missingThumbs: 0,
    bytes: 0,
    estimatedBytes: 0,
    contentVersion: null,
    manga: null,
    seriesReadingDirection: null,
    libraryDefaultReadingDirection: null,
    progress: null,
    createdAt: now(),
    updatedAt: now(),
    completedAt: null,
    ...fields,
  });

  const enqueue = async (record: DownloadRecord) => {
    const stored = await db.update(record.key, (cur) => {
      // Re-downloading at another tier starts over; the same tier resumes.
      if (cur && cur.status === "complete" && cur.tier === record.tier)
        return cur;
      if (cur && cur.tier === record.tier)
        return { ...cur, status: "queued", error: null, errorMessage: null };
      return record;
    });
    if (stored) records.set(record.issueId, stored);
    emit();
    return stored;
  };

  const dropAssets = async (account: string, record: DownloadRecord) => {
    const cs = cacheStorage();
    if (!cs) return;
    const target = await cs.open(offlineCacheName(account));
    const total = Math.max(
      record.pages.length,
      record.pageCount,
      record.donePages,
    );
    const keys: string[] = [offlineCoverKey(record.issueId)];
    for (let i = 0; i < total; i++)
      keys.push(
        offlinePageKey(record.issueId, i),
        offlineStripKey(record.issueId, i),
      );
    await Promise.all(keys.map((k) => target.delete(k)));
  };

  const manager: DownloadManager = {
    async setAccount(account) {
      const previous = await db.getMeta<string>(META_ACCOUNT);
      if (previous !== account) {
        await purgeOthers(account);
        await db.setMeta(META_ACCOUNT, account);
      }
      if (owner !== account) {
        owner = account;
        loaded = loadOwner();
      }
      await loaded;
      void pump();
    },
    async hydrate() {
      if (!loaded) {
        owner = (await db.getMeta<string>(META_ACCOUNT)) ?? null;
        loaded = loadOwner();
      }
      await loaded;
    },
    account: () => owner,
    snapshot: () => snap,
    subscribe(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    onEvent(listener) {
      eventListeners.add(listener);
      return () => eventListeners.delete(listener);
    },
    get: (issueId) => records.get(issueId),
    async downloadIssue(issue, tier, opts) {
      const account = ensureOwner();
      if (opts?.reader) await db.setMeta(META_READER, opts.reader);
      const pages = (issue.pages as PageInfo[] | null | undefined) ?? [];
      await enqueue(
        baseRecord(
          account,
          {
            issueId: issue.id,
            seriesId: issue.series_id,
            seriesSlug: issue.series_slug,
            issueSlug: issue.slug,
            ...detailFields(issue),
            seriesName: opts?.seriesName ?? null,
            pages,
            pageCount: Math.max(pages.length, issue.page_count ?? 0),
            estimatedBytes: estimateIssueBytes(pages, tier, {
              pageCount: issue.page_count,
              fileSize: issue.file_size,
            }),
          },
          tier,
        ),
      );
      halted = false;
      await requestPersistence();
      void pump();
    },
    async downloadSeries(series, tier, opts) {
      const account = ensureOwner();
      if (opts?.reader) await db.setMeta(META_READER, opts.reader);
      await requestPersistence();
      let cursor: string | null | undefined;
      let queued = 0;
      // Walk every cursor page — a series is never assumed to fit one.
      do {
        const qs = new URLSearchParams({ limit: String(SERIES_PAGE_SIZE) });
        if (cursor) qs.set("cursor", cursor);
        const page = await fetchJson<IssueListView>(
          `/series/${encodeURIComponent(series.id)}/issues?${qs}`,
        );
        for (const item of page.items as IssueSummaryView[]) {
          if (item.state !== "active") continue;
          const existing = records.get(item.id);
          if (existing?.status === "complete" && existing.tier === tier)
            continue;
          await enqueue(
            baseRecord(
              account,
              {
                issueId: item.id,
                seriesId: item.series_id,
                seriesSlug: item.series_slug,
                issueSlug: item.slug,
                seriesName: item.series_name ?? series.name ?? null,
                title: item.title ?? null,
                number: item.number ?? null,
                pageCount: item.page_count ?? 0,
                estimatedBytes: estimateIssueBytes([], tier, {
                  pageCount: item.page_count,
                }),
              },
              tier,
            ),
          );
          queued++;
        }
        cursor = page.next_cursor;
      } while (cursor);
      halted = false;
      void pump();
      return queued;
    },
    async pause(issueId) {
      if (active?.issueId === issueId) active.controller.abort();
      await write(issueId, (r) =>
        r.status === "complete" ? {} : { status: "paused" },
      );
    },
    async resume(issueId) {
      halted = false;
      await write(issueId, (r) =>
        r.status === "complete"
          ? {}
          : { status: "queued", error: null, errorMessage: null },
      );
      void pump();
    },
    async remove(issueId) {
      if (!owner) return;
      const account = owner;
      if (active?.issueId === issueId) active.controller.abort();
      const record = records.get(issueId);
      await db.delete(recordKey(account, issueId));
      records.delete(issueId);
      emit();
      if (record) await dropAssets(account, record).catch(() => undefined);
      halted = false;
      void pump();
    },
    async removeAll() {
      if (!owner) return;
      const account = owner;
      active?.controller.abort();
      for (const r of await db.all())
        if (r.account === account) await db.delete(r.key);
      records.clear();
      emit();
      const cs = cacheStorage();
      await cs?.delete(offlineCacheName(account)).catch(() => false);
      await cs?.delete(OFFLINE_SHELL_CACHE).catch(() => false);
      halted = false;
    },
    async clearAll() {
      active?.controller.abort();
      await db.clear();
      records.clear();
      owner = null;
      loaded = null;
      emit();
      const cs = cacheStorage();
      if (!cs) return;
      for (const name of await cs.keys())
        if (
          name.startsWith(OFFLINE_CACHE_PREFIX) ||
          name === OFFLINE_SHELL_CACHE
        )
          await cs.delete(name);
    },
    async verify(issueId) {
      const record = records.get(issueId);
      if (!record || record.status !== "complete" || !owner) return false;
      const target = await cache().catch(() => null);
      if (!target) return false;
      const total = Math.max(record.pages.length, record.pageCount);
      let present = 0;
      for (let i = 0; i < total; i++)
        if (await target.match(offlinePageKey(issueId, i))) present++;
      if (present === total) return true;
      await write(issueId, {
        status: "error",
        error: "unknown",
        errorMessage: "Some pages were removed by the browser. Download again.",
        donePages: present,
      });
      return false;
    },
    async readerPrefs() {
      return (await db.getMeta<ReaderSnapshot>(META_READER)) ?? {};
    },
    async noteProgress(body) {
      if (!records.has(body.issue_id)) return;
      await write(body.issue_id, (r) => ({
        progress: foldProgress(r.progress, body),
      }));
    },
    async usage() {
      const s = storage();
      try {
        const [estimate, persisted] = await Promise.all([
          s?.estimate?.() ?? Promise.resolve(undefined),
          s?.persisted?.() ?? Promise.resolve(null),
        ]);
        return {
          usage: estimate?.usage ?? null,
          quota: estimate?.quota ?? null,
          persisted: persisted ?? null,
        };
      } catch {
        return { usage: null, quota: null, persisted: null };
      }
    },
  };
  return manager;
}

function detailFields(issue: IssueDetailView): Partial<DownloadRecord> {
  return {
    seriesId: issue.series_id,
    seriesSlug: issue.series_slug,
    issueSlug: issue.slug,
    title: issue.title ?? null,
    number: issue.number ?? null,
    contentVersion: issue.last_rewrite_at ?? null,
    manga: issue.manga ?? null,
    seriesReadingDirection: issue.series_reading_direction ?? null,
    libraryDefaultReadingDirection:
      issue.library_default_reading_direction ?? null,
  };
}

let appManager: DownloadManager | undefined;

/** The app-wide download manager (warms the offline shell on completion). */
export function getDownloadManager(): DownloadManager {
  return (appManager ??= createDownloadManager({
    afterComplete: async () => {
      const { warmOfflineShell } = await import("./offline-shell");
      await warmOfflineShell();
    },
  }));
}

export type OfflineResume = {
  initialPage: number;
  initialRun: number;
  restartRun: boolean;
};

/**
 * Resume position for an offline open: the progress captured at download
 * time, the server's (when reachable), and any writes still queued in the
 * outbox, folded with the run rules. Mirrors the reader page's rule that a
 * finished issue parked on its last page reopens from the cover as a new
 * run.
 */
export function resolveOfflineResume(
  record: Pick<DownloadRecord, "progress" | "pageCount" | "pages">,
  queued: ProgressLike[],
  server: ProgressLike | null,
): OfflineResume {
  let progress: OfflineProgress | null = record.progress;
  if (server) progress = foldProgress(progress, server);
  for (const body of queued) progress = foldProgress(progress, body);
  const totalPages = Math.max(1, record.pageCount || record.pages.length);
  if (!progress) return { initialPage: 0, initialRun: 0, restartRun: false };
  const parkedAtEnd = progress.finished && progress.page >= totalPages - 1;
  return {
    initialPage: parkedAtEnd ? 0 : Math.min(progress.page, totalPages - 1),
    initialRun: progress.run,
    restartRun: parkedAtEnd,
  };
}

/**
 * Keep downloaded issues' offline resume positions current: every progress
 * write passes through the durable outbox (WP-4.5) before delivery, so
 * watching it catches reads in the regular reader and in the offline shell
 * alike. Returns an unsubscribe.
 */
export function trackOutboxProgress(
  manager: DownloadManager,
  outbox: Outbox,
): () => void {
  let running = false;
  let again = false;
  const sync = async () => {
    if (running) {
      again = true;
      return;
    }
    running = true;
    try {
      do {
        again = false;
        const entries = await outbox.entries<ProgressLike>({
          kind: PROGRESS_OUTBOX_KIND,
        });
        for (const entry of entries) await manager.noteProgress(entry.payload);
      } while (again);
    } catch {
      /* Best-effort: the next change retries. */
    } finally {
      running = false;
    }
  };
  void sync();
  return outbox.subscribe(() => void sync());
}
