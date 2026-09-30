/**
 * Offline downloads — shared storage contract (WP-4.6).
 *
 * Imported by both the page (download manager, offline library) and the
 * service worker, so it must stay free of DOM-only APIs (no `window`,
 * `localStorage`, React). Everything here is IndexedDB + Cache Storage.
 *
 * Layout
 * - IndexedDB `folio-offline`:
 *   - `downloads` (keyPath `key` = `${account}\u0000${issueId}`): one
 *     `DownloadRecord` per downloaded (or queued) issue — ordered page
 *     metadata, the chosen variant tier, integrity/completeness state and
 *     the reader context captured at download time.
 *   - `meta` (keyPath `name`): `account` — the account that owns the
 *     downloads on this device (set by a signed-in page load; survives
 *     session expiry so a plane ride that outlives the session still
 *     reads; cleared by explicit sign-out) — plus small snapshots.
 * - Cache Storage `folio-offline-v1:<account>`: page bytes and strip/cover
 *   thumbnails under canonical, query-free keys (`offlineKeyFor`), so any
 *   `?w=`/`?v=`/`?r=` variant the reader asks for maps to the downloaded
 *   bytes. Scoped per account by name; the worker reads only the cache of
 *   the owning account.
 * - Cache Storage `folio-offline-shell-v1`: the public (credential-less)
 *   `/downloads` document and the hashed `/_next/static` assets it needs,
 *   so the offline reader shell boots without the network.
 */

import type { PageInfo } from "@/lib/api/types";

export const OFFLINE_DB = "folio-offline";
export const DOWNLOADS_STORE = "downloads";
export const META_STORE = "meta";
export const OFFLINE_CACHE_PREFIX = "folio-offline-v1:";
export const OFFLINE_SHELL_CACHE = "folio-offline-shell-v1";
/** The offline library + reader shell route. */
export const OFFLINE_SHELL_PATH = "/downloads";

export type DownloadTier = 720 | 1080 | 1600 | "original";
export const DOWNLOAD_TIERS: readonly DownloadTier[] = [
  720,
  1080,
  1600,
  "original",
];

export type DownloadStatus =
  /** Waiting for its turn in the queue. */
  | "queued"
  /** Fetching pages now. */
  | "downloading"
  /** Stopped by the user; resumable. */
  | "paused"
  /** Every page stored and verified. The only readable state. */
  | "complete"
  /** Stopped by a failure (quota, network, removed issue); retryable. */
  | "error";

export type DownloadErrorKind = "quota" | "network" | "gone" | "unknown";

export type ReaderSnapshot = {
  default_reading_direction?: string | null;
  default_fit_mode?: string | null;
  default_view_mode?: string | null;
  default_page_strip?: boolean | null;
  default_page_animation?: string | null;
  default_cover_solo?: boolean | null;
  keybinds?: unknown;
  activity_tracking_enabled?: boolean | null;
  reading_min_active_ms?: number | null;
  reading_min_pages?: number | null;
  reading_idle_ms?: number | null;
};

export type DownloadRecord = {
  /** `${account}\u0000${issueId}` */
  key: string;
  account: string;
  issueId: string;
  seriesId: string;
  seriesSlug: string;
  /** Empty until the issue detail is fetched (series enqueue). */
  issueSlug: string;
  seriesName: string | null;
  title: string | null;
  number: string | null;
  tier: DownloadTier;
  status: DownloadStatus;
  error: DownloadErrorKind | null;
  errorMessage: string | null;
  /** Ordered page metadata (`IssueDetailView.pages`); empty until fetched. */
  pages: PageInfo[];
  pageCount: number;
  /** Pages stored and verified so far. */
  donePages: number;
  /** Strip thumbnails that could not be stored (cosmetic; not required). */
  missingThumbs: number;
  /** Bytes stored in Cache Storage for this issue (pages + thumbs). */
  bytes: number;
  estimatedBytes: number;
  /** `issue.last_rewrite_at` at download time — page URL version stamp. */
  contentVersion: string | null;
  manga: string | null;
  seriesReadingDirection: string | null;
  libraryDefaultReadingDirection: string | null;
  /** Server progress captured at download time (the offline resume seed;
   *  queued outbox writes win over it). */
  progress: { page: number; finished: boolean; run: number } | null;
  createdAt: number;
  updatedAt: number;
  completedAt: number | null;
};

export const recordKey = (account: string, issueId: string) =>
  `${account}\u0000${issueId}`;

export const offlineCacheName = (account: string) =>
  `${OFFLINE_CACHE_PREFIX}${account}`;

const PAGE_RE = /^\/issues\/([^/]+)\/pages\/(\d+)$/;
const THUMB_RE = /^\/issues\/([^/]+)\/pages\/(\d+)\/thumb$/;

export const offlinePageKey = (issueId: string, page: number) =>
  `/issues/${encodeURIComponent(issueId)}/pages/${page}`;
export const offlineStripKey = (issueId: string, page: number) =>
  `/issues/${encodeURIComponent(issueId)}/pages/${page}/thumb?variant=strip`;
export const offlineCoverKey = (issueId: string) =>
  `/issues/${encodeURIComponent(issueId)}/pages/0/thumb`;

/**
 * Canonical cache key for a page-bytes / thumbnail URL, dropping the
 * width (`w`), content-version (`v`) and retry (`r`) parameters so any
 * variant the reader asks for resolves to the downloaded bytes. `null`
 * when the path is not a downloadable asset.
 */
export function offlineKeyFor(url: URL): string | null {
  const page = PAGE_RE.exec(url.pathname);
  if (page) return url.pathname;
  const thumb = THUMB_RE.exec(url.pathname);
  if (!thumb) return null;
  const variant = url.searchParams.get("variant");
  if (variant === "strip") return `${url.pathname}?variant=strip`;
  // `cover` / `cover_small` / no variant on page 0 → the stored cover.
  if (thumb[2] === "0") return url.pathname;
  return null;
}

// ────────────── IndexedDB ──────────────

function request<R>(req: IDBRequest<R>): Promise<R> {
  return new Promise((resolve, reject) => {
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error);
  });
}

function done(tx: IDBTransaction): Promise<void> {
  return new Promise((resolve, reject) => {
    tx.oncomplete = () => resolve();
    tx.onabort = tx.onerror = () => reject(tx.error);
  });
}

export interface OfflineDb {
  all(): Promise<DownloadRecord[]>;
  get(key: string): Promise<DownloadRecord | undefined>;
  /** Atomic read-modify-write; `null` from `fn` deletes the record. */
  update(
    key: string,
    fn: (cur: DownloadRecord | undefined) => DownloadRecord | null,
  ): Promise<DownloadRecord | null>;
  delete(key: string): Promise<void>;
  clear(): Promise<void>;
  getMeta<T>(name: string): Promise<T | undefined>;
  setMeta<T>(name: string, value: T | undefined): Promise<void>;
}

type MetaRow = { name: string; value: unknown };

/** In-memory `OfflineDb` (no IndexedDB, and unit tests). */
export function memoryOfflineDb(): OfflineDb {
  const rows = new Map<string, DownloadRecord>();
  const meta = new Map<string, unknown>();
  return {
    async all() {
      return [...rows.values()].map((r) => structuredClone(r));
    },
    async get(key) {
      const row = rows.get(key);
      return row ? structuredClone(row) : undefined;
    },
    async update(key, fn) {
      const next = fn(
        rows.get(key) ? structuredClone(rows.get(key)!) : undefined,
      );
      if (next) rows.set(key, structuredClone(next));
      else rows.delete(key);
      return next;
    },
    async delete(key) {
      rows.delete(key);
    },
    async clear() {
      rows.clear();
      meta.clear();
    },
    async getMeta<T>(name: string) {
      return meta.get(name) as T | undefined;
    },
    async setMeta<T>(name: string, value: T | undefined) {
      if (value === undefined) meta.delete(name);
      else meta.set(name, value);
    },
  };
}

/** IndexedDB-backed `OfflineDb`; falls back to memory when IndexedDB is
 *  unavailable (then nothing survives a reload, and nothing is advertised
 *  as downloaded after one). */
export function indexedOfflineDb(dbName = OFFLINE_DB): OfflineDb {
  let opened: Promise<IDBDatabase | null> | undefined;
  const fallback = memoryOfflineDb();
  const connect = () =>
    new Promise<IDBDatabase | null>((resolve) => {
      if (typeof indexedDB === "undefined") return resolve(null);
      try {
        const req = indexedDB.open(dbName, 1);
        req.onupgradeneeded = () => {
          const db = req.result;
          if (!db.objectStoreNames.contains(DOWNLOADS_STORE))
            db.createObjectStore(DOWNLOADS_STORE, { keyPath: "key" });
          if (!db.objectStoreNames.contains(META_STORE))
            db.createObjectStore(META_STORE, { keyPath: "name" });
        };
        req.onsuccess = () => {
          const db = req.result;
          db.onversionchange = () => {
            db.close();
            opened = undefined;
          };
          resolve(db);
        };
        req.onerror = () => resolve(null);
        req.onblocked = () => resolve(null);
      } catch {
        resolve(null);
      }
    });
  const open = () => (opened ??= connect());
  return {
    async all() {
      const db = await open();
      if (!db) return fallback.all();
      const tx = db.transaction(DOWNLOADS_STORE, "readonly");
      return request(
        tx.objectStore(DOWNLOADS_STORE).getAll() as IDBRequest<
          DownloadRecord[]
        >,
      );
    },
    async get(key) {
      const db = await open();
      if (!db) return fallback.get(key);
      const tx = db.transaction(DOWNLOADS_STORE, "readonly");
      return request(
        tx.objectStore(DOWNLOADS_STORE).get(key) as IDBRequest<
          DownloadRecord | undefined
        >,
      );
    },
    async update(key, fn) {
      const db = await open();
      if (!db) return fallback.update(key, fn);
      const tx = db.transaction(DOWNLOADS_STORE, "readwrite");
      const store = tx.objectStore(DOWNLOADS_STORE);
      const finished = done(tx);
      let result: DownloadRecord | null = null;
      const get = store.get(key) as IDBRequest<DownloadRecord | undefined>;
      get.onsuccess = () => {
        result = fn(get.result);
        if (result) store.put(result);
        else if (get.result) store.delete(key);
      };
      await finished;
      return result;
    },
    async delete(key) {
      const db = await open();
      if (!db) return fallback.delete(key);
      const tx = db.transaction(DOWNLOADS_STORE, "readwrite");
      tx.objectStore(DOWNLOADS_STORE).delete(key);
      await done(tx);
    },
    async clear() {
      const db = await open();
      if (!db) return fallback.clear();
      const tx = db.transaction([DOWNLOADS_STORE, META_STORE], "readwrite");
      tx.objectStore(DOWNLOADS_STORE).clear();
      tx.objectStore(META_STORE).clear();
      await done(tx);
    },
    async getMeta<T>(name: string) {
      const db = await open();
      if (!db) return fallback.getMeta<T>(name);
      const tx = db.transaction(META_STORE, "readonly");
      const row = await request(
        tx.objectStore(META_STORE).get(name) as IDBRequest<MetaRow | undefined>,
      );
      return row?.value as T | undefined;
    },
    async setMeta<T>(name: string, value: T | undefined) {
      const db = await open();
      if (!db) return fallback.setMeta(name, value);
      const tx = db.transaction(META_STORE, "readwrite");
      if (value === undefined) tx.objectStore(META_STORE).delete(name);
      else tx.objectStore(META_STORE).put({ name, value } satisfies MetaRow);
      await done(tx);
    },
  };
}

export const META_ACCOUNT = "account";
export const META_READER = "reader";
export const META_SHELL = "shell";

/**
 * Look up a downloaded asset for the service worker: only from the cache
 * of the account that owns this device's downloads (`meta.account`), and
 * only when that account's record for the issue is complete. Returns
 * `undefined` for anything else so the caller falls through to the network.
 */
export async function matchOfflineAsset(
  db: OfflineDb,
  cacheStorage: CacheStorage,
  url: URL,
): Promise<Response | undefined> {
  const key = offlineKeyFor(url);
  if (!key) return undefined;
  const account = await db.getMeta<string>(META_ACCOUNT);
  if (!account) return undefined;
  const issueId = decodeURIComponent(url.pathname.split("/")[2] ?? "");
  const record = await db.get(recordKey(account, issueId));
  if (!record) return undefined;
  const cache = await cacheStorage.open(offlineCacheName(account));
  return (await cache.match(key, { ignoreVary: true })) ?? undefined;
}
