# Offline reading: separate implementation plan

Not part of PWA hardening. Implemented in this order; steps 1–4 shipped as
**WP-4.6** (per-issue offline download, see "Offline downloads" below) and
step 5 as **WP-4.5** (durable outbox). Step 6 is the open real-device pass.

1. Define account/session ownership and policy for logout, revoked access,
   session expiry, incognito, and shared devices. No cross-account fallback.
   **Done (WP-4.6)** — see "Ownership policy".
2. Build a public offline-capable reader/library shell with explicit local
   account unlock/ownership checks, independent of authenticated SSR/RSC.
   **Done (WP-4.6)** — `/downloads`, a credential-less stored document.
   There is no separate local unlock (PIN/passkey): ownership is the device
   owner recorded at sign-in, and explicit sign-out removes everything.
3. Add Download for offline to issue actions. Persist ordered metadata and
   page assets with integrity/completeness state, resume/cancel/retry controls,
   and an offline library. Never advertise incomplete downloads as readable.
   **Done (WP-4.6)**, for issues and whole series.
4. Add quota estimates, per-issue usage/removal, persistent-storage requests
   after user intent, and graceful denial/eviction handling. Separate transient
   thumbnail eviction from user-requested downloads.
   **Done (WP-4.6)** — Settings → Downloads.
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

## Offline downloads (WP-4.6)

Steps 1–4. Code: `web/lib/pwa/offline-store.ts` (storage contract shared by
the page and the worker), `downloads.ts` (download manager),
`offline-shell.ts` (shell precache), `offline-bootstrap.ts` (signed-in
start-up), `web/app/[locale]/downloads/` (offline library + reader shell),
`web/components/offline/` (download dialog, downloads list), and the
worker routes in `web/app/sw.ts`.

### Storage

- **IndexedDB `folio-offline`**: `downloads` holds one record per issue,
  keyed `account + NUL + issueId` — ordered `pages` metadata
  (`IssueDetailView.pages`), the tier, `status`
  (`queued`/`downloading`/`paused`/`complete`/`error` with a kind: `quota`,
  `network`, `gone`, `unknown`), `donePages`, stored `bytes`, the estimate,
  `missingThumbs`, the reader context the SSR page would have supplied
  (slugs, content version, manga flag, series/library reading direction),
  and an offline resume position. `meta` holds the device owner
  (`account`) and the reader preferences captured at download time.
- **Cache Storage `folio-offline-v1:<account>`**: page bytes, strip
  thumbnails and the cover under canonical keys without `w`/`v`/`r`
  (`offlineKeyFor`), so every `srcSet` pick, content-version stamp and
  retry URL the reader asks for maps to the downloaded bytes. It is
  separate from the transient `folio-thumbs-v3` LRU (1,200 entries), which
  never evicts downloads.
- **Cache Storage `folio-offline-shell-v1`**: the stored `/downloads`
  document plus every hashed `/_next/static` asset it references.

### Download manager

- **Variant tier**: Small 720 / Medium 1080 / Large 1600 px (the FEP-1
  WebP variants, `?w=`) or Original. The default is the tier a fit-width
  portrait read picks on the device; the last choice is remembered. A page
  narrower than the tier is fetched at its original size (never upscaled).
  The dialog shows an estimate (per page: `min(original, tier² × 1.54 ×
  0.2 B)` + 15 KB strip thumbnail; a series uses 24 pages per issue) next to
  `navigator.storage.estimate()`, and warns when it will not fit.
- **Queue**: one issue at a time, three page fetches in flight with
  `priority: "low"`, in the page (never the worker), so a download does not
  compete with the reader and uses the page's cookies. A series download
  walks every cursor page of `/series/{id}/issues` and fetches each issue's
  detail when its turn comes.
- **Resume / cancel / retry**: stored pages are skipped, so pause → resume,
  a reload and a dropped connection all continue where they stopped. A
  download interrupted by unload is re-queued on the next signed-in load;
  `online` re-queues network failures. Remove = cancel + delete.
- **Completeness**: a record is `complete` only when every page is stored
  with an `image/*` body; strip thumbnails and the cover are best-effort
  (`missingThumbs`). The offline library runs `verify` before opening a
  download and demotes one whose pages were evicted ("download again").
- **Quota**: `QuotaExceededError` marks the issue `error: quota` and stops
  the queue until the user acts (resume, remove, a new download). The first
  download asks for `navigator.storage.persist()`; the settings page shows
  whether storage is persistent.
- **Resume position**: every progress write passes through the outbox
  before delivery; `trackOutboxProgress` folds them into the downloaded
  issue's record with the run rules (`foldProgress`), so an offline open
  resumes where the last read — online or offline — stopped. The reader
  shell also folds in queued writes and, when reachable, the server's
  record (`resolveOfflineResume`), and reopens a finished issue from the
  cover as a new run exactly like the SSR page.

### Offline reader shell

- `/downloads` renders nothing from the server: it reads IndexedDB and
  opens a complete download in the regular `Reader` (`?issue=<id>`), with
  `exitUrl` back to the list. The download manager stores a copy fetched
  with `credentials: "omit"` — an anonymous render, so it carries no user
  data — after the first completed download, crawling the HTML, its flight
  data and each chunk/stylesheet for `static/(chunks|css|media)` references
  (lazy chunks are named inside their parent chunk), writing the document
  last and pruning assets the new document no longer references.
- **Worker routes** (`sw.ts`): a navigation whose network fetch fails gets
  the stored shell for `/downloads`, a 302 to
  `/downloads?from=<path>` for any other path when a complete download
  exists (the shell resolves `/read/<series>/<issue>` to the download),
  else the public offline page. Hashed assets missing from the runtime
  cache fall back to the shell cache when the network errors.
  Page/thumbnail requests are answered from the download cache **only
  when their referrer is the `/downloads` document** (synchronous check, so
  every other document keeps the native loader — Range requests, zoom at
  full resolution) and only from the cache of `meta.account` for an issue
  that account has a record for; anything else goes to the network.
- In the shell with no session, an `OutboxReplayer` scoped to the device
  owner replays queued progress once the server is reachable (the signed-in
  root layout mounts its own).

### Ownership policy

- Records and caches are per account. A signed-in page load
  (`OfflineBootstrap` → `setAccount`) records the device owner and purges
  every other account's records and caches, so a second account never
  sees — and the worker never serves — the first account's downloads.
- Explicit sign-out removes all downloads, the shell, and the owner.
- Session expiry keeps them: offline reading must survive a session that
  lapses mid-flight. Anyone holding the device can read them until the
  owner signs out or another account signs in — the same rule as the
  outbox. Shared devices: sign out.
- Revoked access / a lowered age-rating cap: only what the server returned
  was stored; downloads made before a revocation remain readable offline
  until removed or until the next account change. Re-verification against
  the server is not implemented (see gaps).
- Incognito reads in the shell are not offered; the offline reader always
  tracks progress (the user's activity-tracking preference still applies).

### Known gaps (WP-4.6)

- No re-validation of downloads against the server after an archive edit
  (`last_rewrite_at` change) or an ACL / age-rating change; the stale copy
  stays until removed.
- The shell is stored after the first completed download and refreshed on
  each completion; a deploy between downloads leaves the previous (self-
  consistent) shell until the next download completes.
- Markers/bookmarks, OCR, Up Next and the end-of-issue card need the network
  and degrade to empty offline.
- Step 6 (real-device matrix) is not done; the Playwright reader-flow spec
  covers download → offline → read → reconnect → progress replay on
  desktop Chromium only.
