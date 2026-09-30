# Offline reading: separate implementation plan

Not part of PWA hardening. The current offline page makes no download or sync
promise. Implement in this order when this feature is scheduled:

1. Define account/session ownership and policy for logout, revoked access,
   session expiry, incognito, and shared devices. No cross-account fallback.
2. Build a public offline-capable reader/library shell with explicit local
   account unlock/ownership checks, independent of authenticated SSR/RSC.
3. Add Download for offline to issue actions. Persist ordered metadata and
   page assets with integrity/completeness state, resume/cancel/retry controls,
   and an offline library. Never advertise incomplete downloads as readable.
4. Add quota estimates, per-issue usage/removal, persistent-storage requests
   after user intent, and graceful denial/eviction handling. Separate transient
   thumbnail eviction from user-requested downloads.
5. Introduce an account-scoped durable outbox for progress/bookmarks. Define
   idempotency, latest-write ordering, concurrent-device conflict policy,
   schema migration, and reconnect/authentication handling. Show pending/error
   sync state without claiming background synchronization is guaranteed.
   **Shipped ahead of steps 1–4 as WP-4.5** for progress and reading sessions
   (see "Durable outbox" below); bookmarks/markers and the sync-state UI are
   not wired yet.
6. Test airplane-mode cold launch, partial downloads, quota exhaustion,
   permission denial, expired sessions, account switching, app termination,
   corrupted assets, and interrupted updates on real mobile devices.

Keep separate from this plan:

- Push: opt-in UX, VAPID management, subscription persistence/expiry, relevant
  job hooks, notification deep links, and badge clearing/privacy policy.
- File/share intake: supported formats, authenticated pending intake, size/type
  limits, parser validation, duplicate handling, and explicit review before import.

## Durable outbox (WP-4.5)

`web/lib/pwa/outbox.ts` is the reusable queue WP-4.6 builds on. It is
payload-agnostic: each *kind* registers `{ merge?, deliver }`.

- **Storage**: IndexedDB database `folio-outbox`, store `entries` (keyPath
  `id` = `kind` + NUL + `key`). One entry per `(kind, key)`; `enqueue`
  read-modify-writes in one transaction, folding the payload in with the
  kind's `merge` (default newest wins). Replay order is creation order
  (`seq`, kept across merges). No IndexedDB → an in-memory store with the
  same API (no durability).
- **Delivery**: `deliver` returns `done` / `drop` (permanent 4xx) / `retry`
  (offline, 401/403/408/429, 5xx; a throw is a retry, which stops the pass).
  `outcomeForStatus(status)` does the mapping. Removal after `done` is
  conditional on the payload being unchanged, so a value merged in while a
  delivery was in flight is delivered next — at-least-once, handlers must be
  idempotent.
- **Live writers** mirror into the queue (`enqueue` on write,
  `acknowledge(kind, key, payload)` after their own successful POST); the
  queue only matters when the page dies first.
- **Replay** runs in the page (`OutboxReplayer`, mounted in `QueryProvider`
  for a signed-in account) through `apiFetch`, so the CSRF header and token
  refresh behave like live writes. Triggers: launch, `online`, tab visible,
  backoff (5 s → 5 min) while entries are retained, and the service worker's
  `sync` event (tag `folio-outbox`), which only posts `FOLIO_OUTBOX_REPLAY`
  to open windows — a worker cannot read the CSRF cookie or refresh a token,
  and Background Sync is Chromium-only, so it is a wake-up hint, not the
  delivery path. Single-flight per page and, via Web Locks, across tabs.
- **Accounts**: entries are stamped with the user id. A replay delivers only
  the signed-in account's entries and deletes other accounts'. Session expiry
  keeps them for the same account's next sign-in; explicit sign-out replays
  for up to 2 s, then clears the queue.
- **Progress (`progress`)**: key `(issue, target run)`; merge keeps the
  furthest page, sticky `finished`, and a pending restart tag. Every write
  carries its run; a restart carries the run it leaves and the server applies
  it at most once (`api/progress.rs`, idempotent restart), so replays never
  move progress backwards, never open a second run, and writes from a run the
  server has left are ignored (and removed, not retried).
- **Reading sessions (`reading-session`)**: key `client_session_id`, newest
  payload wins; the server's MAX-merge upsert makes replays idempotent; a
  session older than the server's 14-day window is a 422 and is dropped.

Known limits: the replayed progress `updated_at` is the replay time (sessions
keep their client timestamps); the reader's resume page comes from SSR and
does not yet consult queued local writes; there is no pending/error
sync-state UI yet (`outbox.subscribe` + `entries()` are there for it).
