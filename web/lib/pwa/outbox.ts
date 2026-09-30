/**
 * Durable, account-scoped write outbox (WP-4.5, offline plan step 5).
 *
 * A small IndexedDB queue for writes the app must not lose when the tab is
 * killed offline: reading progress and reading sessions today, per-issue
 * offline downloads (WP-4.6) next. The outbox knows nothing about those
 * payloads; each *kind* registers a handler that says how two queued
 * payloads for the same key coalesce and how one is delivered.
 *
 * Model
 * - One entry per `(kind, key)`. `enqueue` read-modify-writes inside one
 *   IndexedDB transaction, folding the new payload into the stored one with
 *   the kind's `merge` (default: newest wins). Keys are the kind's
 *   coalescing unit — progress uses `(issue, run)` so a finished first read
 *   and the re-read that follows it are two entries delivered in order.
 * - Entries replay in creation order (`seq`, kept across merges).
 * - `deliver` returns `"done"` (accepted, remove), `"drop"` (permanently
 *   rejected — a removed issue, a validation error — remove), or `"retry"`
 *   (offline, 5xx, auth: keep, stop this pass). A thrown error is a retry.
 *   `outcomeForStatus` maps an HTTP status onto those.
 * - Removal after delivery is conditional: if a newer payload was merged
 *   into the entry while it was in flight, the entry stays and the next
 *   pass delivers the newer value. Delivery is therefore at-least-once;
 *   handlers must be idempotent (the progress/session endpoints are).
 * - Entries carry the account that queued them. A replay delivers only the
 *   signed-in account's entries (plus untagged ones) and purges every other
 *   account's, so queued writes never cross accounts. Session expiry keeps
 *   them for the same account's next sign-in; explicit sign-out clears.
 *
 * Replay runs in the page, never in the service worker: the CSRF token is
 * a cookie the page reads (`__Host-comic_csrf`), and `apiFetch` owns token
 * refresh. Triggers are launch, `online`, the tab becoming visible, a
 * backoff timer while entries are retained, and — on Chromium — a
 * Background Sync wake-up relayed by the service worker as a
 * `FOLIO_OUTBOX_REPLAY` message to open windows (`startOutboxReplay`).
 *
 * Without IndexedDB (SSR, storage disabled) the outbox falls back to an
 * in-memory store: same API, no durability.
 */

export type DeliveryOutcome = "done" | "drop" | "retry";

export type OutboxEntry<T = unknown> = {
  /** `${kind}\u0000${key}` — the IndexedDB primary key. */
  id: string;
  kind: string;
  key: string;
  /** Account (user id) that queued the write; null when unknown. */
  account: string | null;
  payload: T;
  /** Creation order; replay order. Kept when a payload is merged in. */
  seq: number;
  createdAt: number;
  updatedAt: number;
  /** Delivery attempts that ended in `"retry"`. */
  attempts: number;
  lastError: string | null;
};

export type OutboxKind<T> = {
  /** Fold `incoming` into the queued `stored` payload for the same key.
   *  Must be pure and synchronous (it runs inside a transaction).
   *  Default: `incoming` replaces `stored`. */
  merge?: (stored: T, incoming: T) => T;
  /** Deliver one payload. Throwing counts as `"retry"`. */
  deliver: (payload: T, entry: OutboxEntry<T>) => Promise<DeliveryOutcome>;
};

export type ReplayReport = {
  delivered: number;
  dropped: number;
  /** Entries left queued because a delivery asked to retry. */
  retained: number;
  /** Another tab or an in-flight replay already held the replay lock. */
  skipped: boolean;
};

export type OutboxFilter = { kind?: string };

export interface Outbox {
  /** Queue (or coalesce) a write. Resolves once it is durable. `merge`
   *  overrides the registered kind's merge (for writers that enqueue
   *  before a handler is registered in this page). */
  enqueue<T>(
    kind: string,
    key: string,
    payload: T,
    opts?: { merge?: (stored: T, incoming: T) => T },
  ): Promise<void>;
  /** Remove the entry if its payload still equals `delivered` — call after
   *  a write delivered outside `replay` (the live reader path). */
  acknowledge<T>(kind: string, key: string, delivered: T): Promise<boolean>;
  entries<T = unknown>(filter?: OutboxFilter): Promise<OutboxEntry<T>[]>;
  /** Deliver the current account's queued entries. Single-flight within a
   *  page and (where `navigator.locks` exists) across tabs. */
  replay(): Promise<ReplayReport>;
  /** Drop every entry (explicit sign-out). */
  clear(): Promise<void>;
  register<T>(kind: string, handler: OutboxKind<T>): () => void;
  /** The account new entries are stamped with and replays deliver for. */
  setAccount(account: string | null | undefined): void;
  /** Notified after every change to the queue (for sync-state UI). */
  subscribe(listener: () => void): () => void;
}

/** Map an HTTP status to a delivery outcome: 2xx done; auth, timeout,
 *  rate-limit, and 5xx retry; any other 4xx is permanent and dropped. */
export function outcomeForStatus(status: number): DeliveryOutcome {
  if (status >= 200 && status < 300) return "done";
  if (status === 401 || status === 403 || status === 408 || status === 429)
    return "retry";
  if (status >= 500) return "retry";
  return "drop";
}

// ────────────── Storage ──────────────

type Update<T> = (current: OutboxEntry<T> | undefined) => OutboxEntry<T> | null;

export interface OutboxStorage {
  all(): Promise<OutboxEntry[]>;
  /** Atomic read-modify-write of one entry; `null` from `fn` deletes it. */
  update<T>(id: string, fn: Update<T>): Promise<void>;
  clear(): Promise<void>;
}

export function memoryStorage(): OutboxStorage {
  const rows = new Map<string, OutboxEntry>();
  return {
    async all() {
      return [...rows.values()].map((row) => structuredClone(row));
    },
    async update<T>(id: string, fn: Update<T>) {
      const next = fn(rows.get(id) as OutboxEntry<T> | undefined);
      if (next) rows.set(id, structuredClone(next) as OutboxEntry);
      else rows.delete(id);
    },
    async clear() {
      rows.clear();
    },
  };
}

const STORE = "entries";

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

/** IndexedDB-backed storage; falls back to memory when IndexedDB cannot
 *  be opened. */
export function indexedDbStorage(dbName = "folio-outbox"): OutboxStorage {
  let opened: Promise<IDBDatabase | null> | undefined;
  const fallback = memoryStorage();
  // Open at the current version; if the store is missing (the database
  // was created by something else, e.g. a devtools/e2e probe), bump the
  // version once to create it.
  const connect = (version?: number) =>
    new Promise<IDBDatabase | null>((resolve) => {
      if (typeof indexedDB === "undefined") return resolve(null);
      try {
        const req = indexedDB.open(dbName, version);
        req.onupgradeneeded = () => {
          if (!req.result.objectStoreNames.contains(STORE))
            req.result.createObjectStore(STORE, { keyPath: "id" });
        };
        req.onsuccess = () => {
          const db = req.result;
          if (!db.objectStoreNames.contains(STORE)) {
            const next = db.version + 1;
            db.close();
            return resolve(version === undefined ? connect(next) : null);
          }
          // Another tab upgrading the schema: step aside, reopen lazily.
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
      const tx = db.transaction(STORE, "readonly");
      const rows = await request(
        tx.objectStore(STORE).getAll() as IDBRequest<OutboxEntry[]>,
      );
      return rows;
    },
    async update<T>(id: string, fn: Update<T>) {
      const db = await open();
      if (!db) return fallback.update(id, fn);
      const tx = db.transaction(STORE, "readwrite");
      const store = tx.objectStore(STORE);
      const finished = done(tx);
      // Stay inside the request callback so the transaction is still
      // active for the write (no promise hop between get and put).
      const get = store.get(id) as IDBRequest<OutboxEntry<T> | undefined>;
      get.onsuccess = () => {
        const next = fn(get.result);
        if (next) store.put(next);
        else if (get.result) store.delete(id);
      };
      await finished;
    },
    async clear() {
      const db = await open();
      if (!db) return fallback.clear();
      const tx = db.transaction(STORE, "readwrite");
      tx.objectStore(STORE).clear();
      await done(tx);
    },
  };
}

// ────────────── Outbox ──────────────

const idOf = (kind: string, key: string) => `${kind}\u0000${key}`;
const same = (a: unknown, b: unknown) =>
  JSON.stringify(a) === JSON.stringify(b);

export const ACCOUNT_STORAGE_KEY = "folio:account-id";

function storedAccount(): string | null {
  try {
    const id = globalThis.localStorage?.getItem(ACCOUNT_STORAGE_KEY);
    return id && id !== "anonymous" ? id : null;
  } catch {
    return null;
  }
}

type Locks = {
  request<R>(
    name: string,
    options: { ifAvailable: boolean },
    callback: (lock: unknown) => Promise<R>,
  ): Promise<R>;
};

export function createOutbox(
  opts: {
    storage?: OutboxStorage;
    now?: () => number;
    /** Web Locks name for cross-tab single-flight replay. */
    lockName?: string;
  } = {},
): Outbox {
  const storage = opts.storage ?? indexedDbStorage();
  const now = opts.now ?? Date.now;
  const lockName = opts.lockName ?? "folio-outbox-replay";
  const handlers = new Map<string, OutboxKind<unknown>>();
  const listeners = new Set<() => void>();
  let account: string | null | undefined;
  let counter = 0;
  let running: Promise<ReplayReport> | undefined;

  const currentAccount = () =>
    account === undefined ? storedAccount() : account;
  const emit = () => {
    for (const listener of listeners) listener();
  };
  const nextSeq = () => now() * 1000 + (counter++ % 1000);

  const pass = async (): Promise<ReplayReport> => {
    const report: ReplayReport = {
      delivered: 0,
      dropped: 0,
      retained: 0,
      skipped: false,
    };
    const me = currentAccount();
    // A few passes pick up payloads merged in while a delivery was in
    // flight; bounded so a hot writer can't pin the loop.
    for (let round = 0; round < 4; round++) {
      const rows = (await storage.all()).sort((a, b) => a.seq - b.seq);
      let progressed = false;
      let stop = false;
      for (const entry of rows) {
        if (me !== null && entry.account !== null && entry.account !== me) {
          // Another account's leftovers: never deliver across accounts.
          await storage.update(entry.id, () => null);
          continue;
        }
        const handler = handlers.get(entry.kind);
        if (!handler) continue;
        let outcome: DeliveryOutcome;
        let error: string | null = null;
        try {
          outcome = await handler.deliver(entry.payload, entry);
        } catch (e) {
          outcome = "retry";
          error = e instanceof Error ? e.message : String(e);
        }
        if (outcome === "retry") {
          await storage.update(entry.id, (cur) =>
            cur
              ? {
                  ...cur,
                  attempts: cur.attempts + 1,
                  lastError: error ?? "retry",
                }
              : null,
          );
          report.retained++;
          stop = true;
          break;
        }
        await storage.update(entry.id, (cur) =>
          !cur || outcome === "drop" || same(cur.payload, entry.payload)
            ? null
            : cur,
        );
        if (outcome === "done") report.delivered++;
        else report.dropped++;
        progressed = true;
      }
      if (stop || !progressed) break;
    }
    return report;
  };

  const locked = (): Promise<ReplayReport> => {
    const locks = (
      globalThis.navigator as (Navigator & { locks?: Locks }) | undefined
    )?.locks;
    if (!locks) return pass();
    return locks.request(lockName, { ifAvailable: true }, async (lock) =>
      lock ? pass() : { delivered: 0, dropped: 0, retained: 0, skipped: true },
    );
  };

  return {
    async enqueue<T>(
      kind: string,
      key: string,
      payload: T,
      enqueueOpts?: { merge?: (stored: T, incoming: T) => T },
    ) {
      const id = idOf(kind, key);
      const merge =
        enqueueOpts?.merge ??
        ((handlers.get(kind)?.merge ?? null) as
          ((stored: T, incoming: T) => T) | null);
      const stamp = now();
      const owner = currentAccount();
      await storage.update<T>(id, (cur) =>
        cur
          ? {
              ...cur,
              payload: merge ? merge(cur.payload, payload) : payload,
              updatedAt: stamp,
            }
          : {
              id,
              kind,
              key,
              account: owner,
              payload,
              seq: nextSeq(),
              createdAt: stamp,
              updatedAt: stamp,
              attempts: 0,
              lastError: null,
            },
      );
      emit();
    },
    async acknowledge<T>(kind: string, key: string, delivered: T) {
      let removed = false;
      await storage.update<T>(idOf(kind, key), (cur) => {
        if (cur && same(cur.payload, delivered)) {
          removed = true;
          return null;
        }
        return cur ?? null;
      });
      if (removed) emit();
      return removed;
    },
    async entries<T = unknown>(filter?: OutboxFilter) {
      const rows = (await storage.all()) as OutboxEntry<T>[];
      return rows
        .filter((row) => !filter?.kind || row.kind === filter.kind)
        .sort((a, b) => a.seq - b.seq);
    },
    replay() {
      if (running) return running;
      running = locked().finally(() => {
        running = undefined;
        emit();
      });
      return running;
    },
    async clear() {
      await storage.clear();
      emit();
    },
    register<T>(kind: string, handler: OutboxKind<T>) {
      handlers.set(kind, handler as OutboxKind<unknown>);
      return () => {
        if (handlers.get(kind) === (handler as OutboxKind<unknown>))
          handlers.delete(kind);
      };
    },
    setAccount(next) {
      account = next ?? null;
    },
    subscribe(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
  };
}

// ────────────── App singleton + replay triggers ──────────────

let appOutbox: Outbox | undefined;

/** The app-wide outbox (lazily opens `folio-outbox` in IndexedDB). */
export function getOutbox(): Outbox {
  return (appOutbox ??= createOutbox());
}

/** Best-effort replay bounded by `timeoutMs` — before an explicit
 *  sign-out, which then `clear()`s whatever could not be delivered. */
export async function replayWithin(
  outbox: Outbox,
  timeoutMs: number,
): Promise<void> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  await Promise.race([
    outbox.replay().catch(() => undefined),
    new Promise<void>((resolve) => {
      timer = setTimeout(resolve, timeoutMs);
    }),
  ]);
  clearTimeout(timer);
}

export const OUTBOX_SYNC_TAG = "folio-outbox";
export const OUTBOX_REPLAY_MESSAGE = "FOLIO_OUTBOX_REPLAY";

type SyncRegistration = ServiceWorkerRegistration & {
  sync?: { register(tag: string): Promise<void> };
};

/** Ask the service worker for a Background Sync wake-up (Chromium only).
 *  The SW does not deliver anything itself — it messages open windows,
 *  which replay through `apiFetch` with the CSRF header. */
async function requestBackgroundSync(): Promise<void> {
  try {
    const sw = globalThis.navigator?.serviceWorker;
    if (!sw?.controller) return;
    const registration = (await sw.ready) as SyncRegistration;
    await registration.sync?.register(OUTBOX_SYNC_TAG);
  } catch {
    /* Unsupported or denied: page triggers still apply. */
  }
}

const BACKOFF_MS = [5_000, 15_000, 30_000, 60_000, 120_000, 300_000];

/**
 * Wire the page-side replay triggers: launch (now), `online`, the tab
 * becoming visible, a backoff timer while deliveries are being retried,
 * and the service worker's Background Sync relay. Returns a cleanup.
 */
export function startOutboxReplay(
  outbox: Outbox,
  opts: { onReport?: (report: ReplayReport) => void } = {},
): () => void {
  let stopped = false;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let failures = 0;
  const run = async () => {
    if (stopped) return;
    if (timer) clearTimeout(timer);
    timer = undefined;
    const report = await outbox.replay().catch((): ReplayReport => ({
      delivered: 0,
      dropped: 0,
      retained: 1,
      skipped: false,
    }));
    if (stopped) return;
    opts.onReport?.(report);
    if (report.retained > 0) {
      const delay = BACKOFF_MS[Math.min(failures, BACKOFF_MS.length - 1)]!;
      failures++;
      timer = setTimeout(() => void run(), delay);
      void requestBackgroundSync();
    } else if (!report.skipped) {
      failures = 0;
    }
  };
  const trigger = () => void run();
  const onVisible = () => {
    if (document.visibilityState === "visible") trigger();
  };
  const onMessage = (event: MessageEvent) => {
    if (event.data?.type === OUTBOX_REPLAY_MESSAGE) trigger();
  };
  const sw = globalThis.navigator?.serviceWorker;
  window.addEventListener("online", trigger);
  document.addEventListener("visibilitychange", onVisible);
  sw?.addEventListener("message", onMessage);
  trigger();
  return () => {
    stopped = true;
    if (timer) clearTimeout(timer);
    window.removeEventListener("online", trigger);
    document.removeEventListener("visibilitychange", onVisible);
    sw?.removeEventListener("message", onMessage);
  };
}
