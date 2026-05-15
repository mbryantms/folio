# Incompleteness Audit

**Date:** 2026-05-15
**Scope:** Whole repo — Rust workspace under [crates/](../../crates/) and Next.js app under [web/](../../web/). Excluded `node_modules/`, `target/`, `.next/`, `.dev-data/`, generated files (`web/lib/api/types.generated.ts`, `web/lib/api/openapi.json`).
**Method:** Four parallel static sweeps — code markers, server-side stubs, web UI gaps, feature-flag / dead-code patterns. Cross-referenced against project memory (active plans, deferred items) before flagging.
**Verdict:** The codebase is in noticeably good shape. Most of what looks unfinished is **intentional deferral with documentation** (phase-2 placeholders pending Phase-4 Automerge sync, archive-format stubs scoped out of v1, single-milestone reader-feature carryovers). Two real gaps need attention; everything else is either documented or trivially fixable.

> Caveat: this is a static-grep + structural audit. It won't catch behavioral bugs, semantic gaps inside otherwise-wired features, or anything that compiles cleanly but is wrong. Treat as a starting-point checklist, not a completeness proof.

## Severity legend

- **Blocking** — violates a project invariant (CLAUDE.md rule, audit-log discipline, ACL gate) or a documented contract. Should be fixed before any v1-ready claim.
- **Should-fix** — small, visible, low-cost. No defensible reason to defer.
- **Deferred-by-design** — wired up or stubbed deliberately; documented in code, memory, or planning artifacts. Track via existing plan/milestone, don't re-plan in this audit.
- **Trivial** — comment rot, doc gaps, one-line nits.

---

## 1. Blocking

### B-1. `POST /admin/queue/clear` missing audit-log entry — CLOSED 2026-05-15 (cleanup M1)

**File:** [crates/server/src/api/admin_queue.rs:93](../../crates/server/src/api/admin_queue.rs#L93)
**Severity:** Blocking — violates the "every mutating admin handler emits via `crate::audit::record`" rule in [CLAUDE.md](../../CLAUDE.md).
**What's missing:** The handler clears apalis job queues via Redis and marks thumb-job rows in the DB, but never calls `audit::record`. Every other mutating admin handler in the codebase emits.
**Recommended action:** Add an `audit::record` call with `action: "admin.queue.clear"`, `target_type: Some("queue")`, and a payload carrying the queue names cleared plus the before/after depth counts. Append-only invariant for the audit log means this is unrecoverable history if the endpoint is used without it.

### B-2. On Deck rail `cbl_next` card opens reader without CBL context — CLOSED 2026-05-15 (cleanup M3)

**Files:**
- Server response shape: [crates/server/src/api/rails.rs](../../crates/server/src/api/rails.rs) (`OnDeckCard::CblNext` carries `cbl_list_id` only).
- Web consumer: [web/components/library/OnDeckCard.tsx:107,119,137](../../web/components/library/OnDeckCard.tsx#L107).

**Severity:** Blocking — breaks the contract established by the reader-end-of-issue plan that CBL context flows via `?cbl=<saved_view_id>` on every CBL-source reader link.
**What's missing:** The home On Deck rail surfaces a CBL-source card, but clicking it routes through `readerUrl(issue)` (no `cbl` opt). The reader's next-up resolver then picks series-next instead of staying in CBL. The card UI implies "you're reading this CBL" but the URL doesn't propagate that context.
**Why it's stuck:** `OnDeckCard::CblNext` exposes `cbl_list_id` (the raw CBL list id) but not the `saved_view_id` of the CBL saved view that wraps it. The web layer's `cbl` URL contract is keyed on saved-view ids, not list ids. Fixing requires a saved-view-by-cbl-list-id resolution somewhere — either in the on-deck handler (server-side join) or via a client-side lookup.
**Recommended action:** Add `cbl_saved_view_id: Option<String>` to the `OnDeckCard::CblNext` variant; populate it in the on-deck handler by joining `cbl_lists` → `saved_views` (kind='cbl', cbl_list_id=…, owned by the calling user OR system). Web threads it onto both `readerUrl()` and `issueUrl()` calls in `OnDeckCard.tsx`. Carried over as a known gap in the M2 + M5 reader-end-of-issue done memos.

---

## 2. Should-fix

### S-1. Two undocumented environment variables — CLOSED 2026-05-15 (cleanup M1)

**Files:**
- `COMIC_LOAD_DOTENV` — [crates/server/src/main.rs:17](../../crates/server/src/main.rs#L17). Gates `.env` loading in debug builds.
- `COMIC_GITHUB_TOKEN` — [crates/server/src/cbl/catalog.rs:80](../../crates/server/src/cbl/catalog.rs#L80). Raises CBL-catalog GitHub rate limit from 60 → 5000 req/hr when set.

**Severity:** Should-fix — both are silent footguns for self-hosters. The `COMIC_*` namespace convention implies discoverability via [.env.example](../../.env.example) and [docs/dev/runtime-configuration.md](runtime-configuration.md).
**Recommended action:** Add both to `.env.example` with a one-line description; cross-link in `runtime-configuration.md`'s env-var matrix.

### S-2. Stale "501 stubs" comment in `auth/local.rs` — CLOSED 2026-05-15 (cleanup M1)

**File:** [crates/server/src/auth/local.rs:12](../../crates/server/src/auth/local.rs#L12)
**Severity:** Trivial leaning Should-fix — actively misleading to anyone scanning the module for the recovery flow.
**What's wrong:** Module comment reads "endpoint stubs return 501" but the recovery endpoints (`verify-email`, `resend-verification`, `request-password-reset`, `reset-password`) all shipped in **auth-hardening M4**. The comment hasn't been updated.
**Recommended action:** Delete the stale clause; the rest of the module comment is accurate.

### S-3. Six `#[allow(dead_code)]` instances outside test code — CLOSED 2026-05-15 (cleanup M1)

**Files:**
- [crates/server/src/api/series.rs:1794](../../crates/server/src/api/series.rs#L1794)
- [crates/server/src/auth/csrf.rs:201](../../crates/server/src/auth/csrf.rs#L201)
- [crates/server/src/api/cbl_lists.rs:1022](../../crates/server/src/api/cbl_lists.rs#L1022)
- [crates/server/src/auth/oidc.rs:695](../../crates/server/src/auth/oidc.rs#L695)
- [crates/server/src/auth/cookies.rs:96](../../crates/server/src/auth/cookies.rs#L96), [:127](../../crates/server/src/auth/cookies.rs#L127)
- [crates/server/src/email/mod.rs:149](../../crates/server/src/email/mod.rs#L149)

**Severity:** Should-fix — each instance is either rot (delete) or load-bearing-but-undocumented (annotate). The current state forces every future reader to context-switch and ask "is this real?"
**Recommended action:** Per site — confirm whether the marked field/symbol is referenced anywhere, then either remove it or add a one-line `// <reason>` comment above the `#[allow]` explaining why it's kept.

### S-4. Re-verify `IssueSettingsMenu.tsx` "Coming soon" comment — CLOSED 2026-05-15 (cleanup M1)

**File:** [web/app/[locale]/(library)/series/[slug]/issues/[issueSlug]/IssueSettingsMenu.tsx:67](../../web/app/[locale]/(library)/series/[slug]/issues/[issueSlug]/IssueSettingsMenu.tsx#L67)
**Severity:** Should-fix — comment is partially stale.
**What's wrong:** Comment claims the menu "stubs out the 'Coming soon' affordances (favorite, bookmark, reading list, collection, download)". But bookmark + collection both ship via the markers (M6/M7) and collections (Markers+Collections plan) work. Some of the listed affordances are still genuinely missing; others have landed since the comment was written.
**Recommended action:** Audit the current menu contents vs. the listed-as-coming-soon items. Update the comment to reflect reality; if any of `favorite` / `reading list` / `download` should now be wired, file follow-ups.

---

## 3. Deferred-by-design (track, don't re-plan)

### D-1. CBR archive format support — SCHEDULED 2026-05-15 (separate plan needed)

**Files:** [crates/archive/src/cbr.rs:25,43](../../crates/archive/src/cbr.rs#L25)
**What's there:** Both `open()` and `read_entry_bytes()` return `Malformed("CBR support not yet implemented")`.
**Decision (2026-05-15, cleanup M2):** **Ship rar + 7z support.** Bundled with D-2 since the libraries / scanner integration / error UX overlap. Too much scope for a milestone slot in the cleanup plan; needs its own driver.
**Action:** File `~/.claude/plans/archive-formats-1.0.md` when ready to begin. Tracked via memory entry `incompleteness_cleanup_m2_done`.

### D-2. CB7 archive format support — SCHEDULED 2026-05-15 (separate plan needed)

**Files:** [crates/archive/src/cb7.rs:19,37](../../crates/archive/src/cb7.rs#L19)
**Status:** Same shape as D-1; bundled with it into the same future plan per the M2 decision.

### D-3. Dictionary "did you mean" trigram refresh — CLOSED 2026-05-15 (deferred to search v1.1)

**File:** [crates/server/src/jobs/post_scan.rs:817](../../crates/server/src/jobs/post_scan.rs#L817)
**What's there:** `post_scan_dictionary` job is enqueued but the function body is an empty stub with a `TODO` for the trigram refresh.
**Decision (2026-05-15, cleanup M2):** Punt to search v1.1. Search functions fine without "did you mean" today; revisit when search-quality concerns surface.
**Action taken:** Added a deferral note to [library-scanner.md §Dictionary refresh](library-scanner.md) cross-linking back to this finding.

### D-4. Light + amber theme palettes — CLOSED 2026-05-15 (cleanup M6)

**Files changed:** [web/styles/globals.css](../../web/styles/globals.css), [web/lib/theme.ts](../../web/lib/theme.ts), [web/components/settings/ThemePicker.tsx](../../web/components/settings/ThemePicker.tsx), [web/tests/admin/theme.test.ts](../../web/tests/admin/theme.test.ts)
**What's there:** Light was already fully wired in `globals.css` (`[data-theme="light"]` block existed from M4 scaffolding); the gap was just the `pickTheme` short-circuit firing `toast.info("Coming soon...")`. Amber needed a new `[data-theme="amber"]` block + a `resolvedDataTheme()` branch.
**Decision (2026-05-15, cleanup M2):** Curate the palettes now.
**Action taken:**

- Added `[data-theme="amber"]` block in `globals.css` — sepia / paper-warmth palette inspired by Kindle/Kobo warm-light reading modes. Warm cream background (HSL `38 35% 93%`), warm-brown ink, burnt-amber primary. Different enough from Light (clean white) to be a useful third option.
- `resolvedDataTheme()` now branches per palette: `light → light`, `amber → amber`, everything else (`system`, `dark`, nullish) `→ dark`. System still maps to dark because `ThemeProvider` is `enableSystem={false}`; flipping that needs separate FOUC-on-hydration work (out of scope).
- Dropped the "Coming soon" toast + comment in `ThemePicker.pickTheme`. Updated the SettingsSection description to describe what each palette actually is.
- Extended `theme.test.ts` with explicit "each curated theme maps to its own data-theme attribute" assertion covering light + amber.

### D-5. `/admin/search` placeholder — CLOSED 2026-05-15 (dropped)

**Files (deleted in cleanup M2):**

- Nav entry: row removed from [web/components/admin/nav.ts](../../web/components/admin/nav.ts).
- Route: `web/app/[locale]/(admin)/admin/search/page.tsx` deleted.
- Placeholder component (single-use): `web/components/admin/PlaceholderPage.tsx` deleted.

**Decision (2026-05-15, cleanup M2):** Drop the nav entry + route + retire PlaceholderPage. No real plan for admin search today; add back later if one materializes.
**Action taken:** All three files removed; `.next/` build cache nuked to clear stale route artifacts. Typecheck + 282 vitest green.

### D-6. `NextUpView.fallback_suggestion` always serialized as `None` — CLOSED 2026-05-15 (cleanup M3)

**File:** [crates/server/src/api/next_up.rs](../../crates/server/src/api/next_up.rs)
**Status:** Reserved field on the response contract; population deferred from reader-end-of-issue M5 because the `rails::on_deck` composition isn't factored as a reusable "top On Deck card" helper yet. The web layer renders a static "Browse the library" CTA instead, which the plan acknowledged as the no-on-deck fallback.
**Action when revisited:** Refactor `rails::on_deck` to expose a `top_on_deck_card(app, user, acl) -> Option<OnDeckCard>` helper; call from `next_up` when `source == "none"`. Renders a single suggested-next tile in the end-of-issue card's caught-up state.

### D-7. Reader `prevIssue` keybind / endpoint — CLOSED 2026-05-15 (cleanup M4)

**Status:** Listed as deferred in the reader-end-of-issue plan. M2 shipped `nextIssue` (`Shift+N`); `prevIssue` (`Shift+P`) wasn't bundled because it needs either a "previous-up" server endpoint or client-side derivation from a series query cache.
**Action when revisited:** Symmetric `prev-up` endpoint OR client-side lookup from existing data; register `prevIssue` action with `Shift+P` default.

### D-8. `comic_reader_next_up_latency_seconds` histogram — CLOSED 2026-05-15 (cleanup M3)

**File:** [crates/server/src/api/next_up.rs](../../crates/server/src/api/next_up.rs)
**Status:** The reader-end-of-issue M5 plan called for both a counter (shipped: `comic_reader_next_up_resolved_total{source}`) and a latency histogram (not shipped). The histogram was punted as "defer until perf becomes a real concern".
**Action when revisited:** Wrap the handler body with a `metrics::histogram!` timer; expose buckets sized for the series-walk worst case (large libraries).

### D-9. OTLP exporter wiring — RESOLVED 2026-05-15: considered, not chosen

**Files:** [crates/server/src/observability.rs:325-345](../../crates/server/src/observability.rs#L325), [.env.example](../../.env.example) (`COMIC_OTLP_ENDPOINT`)
**What was planned:** Ship a `tracing-opentelemetry` exporter behind a runtime-config flag in the `app_setting` registry so admins could enable OTLP shipping without restart.
**Decision:** Reconsidered and dropped on 2026-05-15. The reasoning:

- The `opentelemetry` / `opentelemetry-otlp` / `opentelemetry_sdk` / `tracing-opentelemetry` crate stack has a notoriously volatile compat matrix (frequent yanks, major API breaks between minor versions). Maintenance overhead is recurring.
- No user has reported needing OTLP. Prometheus `/metrics` already covers the operator-monitoring use case for self-hosted deployments.
- The runtime-config-admin slice for OTLP would still need its own admin-UI design pass and migration.

**What replaces it:** The existing `COMIC_OTLP_ENDPOINT` env var stays read by `Config` (no breaking change for anyone with it set), but the observability boot path now logs a clear "intentionally not shipped in v1" message instead of "deferred to a later sub-phase" when the var is set.
**Re-evaluation trigger:** A hosted Folio deployment ships (would need remote-traces shipping), OR a user reports a real need for OTLP that Prometheus `/metrics` doesn't cover. At that point, look at simpler alternatives first (e.g., a Prometheus push gateway, or a Loki/Vector sidecar) before re-adding the OpenTelemetry dep chain.
**Status:** Closed.

### D-10. `COMIC_ARCHIVE_MAX_*` enforcement — CLOSED 2026-05-15 (cleanup M5)

**File:** [.env.example:74-82](../../.env.example#L74)
**What's there:** Env vars `COMIC_ARCHIVE_MAX_BYTES`, `COMIC_ARCHIVE_MAX_PAGES`, `COMIC_ARCHIVE_MAX_PAGE_BYTES` are declared and documented as "spec'd but unenforced."
**Status:** Documented intentional gap. The scanner reads file sizes but doesn't reject oversized archives.
**Action when revisited:** Plumb the caps through the archive open / page-bytes paths; reject scans / page fetches that exceed.

### D-11. `progress_records` → ~~Phase-4 Automerge sync~~ — RESOLVED 2026-05-15: considered, not chosen

**Files:** [crates/entity/src/progress_record.rs](../../crates/entity/src/progress_record.rs), [crates/migration/src/m20260201_000003_progress_placeholder.rs](../../crates/migration/src/m20260201_000003_progress_placeholder.rs), [crates/server/src/api/progress.rs](../../crates/server/src/api/progress.rs)
**What was planned:** Spec §9 (original) called for replacing the `progress_records` table with per-user Automerge CRDT documents in Phase 4, with WebSocket sync, custom merge rules, compaction workers, sharding, and a 90-day cutover migration.
**Decision:** Reconsidered and dropped on 2026-05-15. The conflicts Automerge was specified to resolve don't exist in practice — progress is a single monotonic scalar that the server already resolves with `max(last_page)`; bookmarks/annotations were solved without CRDTs via the markers schema (M1–M8); shared collections are explicitly owner-authoritative (spec §9.5), bypassing the CRDT collaboration use case. The operational cost of the CRDT path (BYTEA storage, WebSocket auth handshake, compaction, sharding, multi-language client bindings, custom merge code) is real and recurring; the benefit is theoretical until a native client with offline editing actually ships.
**What replaces it:** Nothing — the current `progress_records` system is the long-term store. Spec §9 has been updated with the decision note; entity / migration / handler comments updated to drop the "placeholder" framing.
**Re-evaluation trigger:** A native client (Phase 5 desktop or Phase 6 mobile) actively being built where offline editing of reading state is a real product requirement. At that point, re-evaluate Automerge against simpler alternatives (service-worker outbox, RxDB-style sync, WebSocket push of server-resolved deltas).
**Status:** Closed.

---

## 4. Verified clean

These were checked and produced no findings:

- **Endpoint wiring** — no orphans either direction between `app.rs` utoipa `paths(...)` and `router()` merges.
- **Background job kinds** — every `apalis::WorkerBuilder` has an enqueue site, every enqueue site has a worker.
- **`app_setting` registry** — every key is read by application code; no DB-write-only entries.
- **Auth-mode flow consistency** — `Local` / `OIDC` / `Both` paths gated symmetrically server- and client-side.
- **OpenAPI schema completeness** — every `responses(body = T)` registers `T` in `components(schemas)`.
- **Migration ↔ entity drift** — spot-checked recent migrations; all matched.
- **Test scaffolding** — no fixture data, mocks, or dev credentials escape `tests/` directories.
- **Disabled-button discipline** — every `disabled={…}` ties to a real `isPending` / `!isDirty` / search-active state.
- **Form-field bindings** — sampled forms (`LibrarySettingsForm`, `KeybindEditor`, theme picker, collection CRUD) all serialize cleanly.
- **Mutation hooks consumed** — every `useApiMutation` export has a call site.
- **Loading / error / empty states** on major data consumers (list views, admin dashboards, series detail) — comprehensive.
- **No `#[ignore]`, `.skip()`, `.todo()`** in test suites.
- **No `console.log` / `dbg!`** in committed source.
- **No `eslint-disable` blocks** on production TS/TSX.
- **No commented-out code blocks** (3+ consecutive lines).
- **No demo / `/dev/*` / `/playground` routes.**

---

## 5. Missing-implementation matrix

Status: ✅ shipped · ◐ partial / wired-but-deferred · ❌ stub · n/a not applicable.

| Finding | Server | Web UI | Tests | Docs | Severity |
|---|---|---|---|---|---|
| **B-1** ~~`clear_queue` audit log~~ | ✅ `audit::record` wired | n/a | ✅ 3 new integration tests | n/a | **Closed M1** |
| **B-2** ~~OnDeck `cbl_next` → reader CBL context~~ | ✅ `cbl_saved_view_id` populated via saved-view join | ✅ `cblParam` threaded into `readerUrl()` / `issueUrl()` | ✅ 3 new server + 4 new vitest tests | ✅ OnDeckCard type updated | **Closed M3** |
| **S-1** ~~`COMIC_LOAD_DOTENV` / `COMIC_GITHUB_TOKEN` docs~~ | ✅ both read | n/a | n/a | ✅ documented in `.env.example` + runtime-config | **Closed M1** |
| **S-2** ~~`auth/local.rs:12` "501 stubs" comment~~ | ✅ endpoints shipped | ✅ wired | ✅ covered | ✅ comment fixed | **Closed M1** |
| **S-3** ~~6× `#[allow(dead_code)]` in prod~~ | ✅ 3 kept w/ comments, 3 deleted | n/a | n/a | n/a | **Closed M1** |
| **S-4** ~~`IssueSettingsMenu.tsx` comment~~ | n/a | ✅ comment rewritten to reflect reality | n/a | ✅ accurate | **Closed M1** |
| **D-1** CBR archive | ❌ stub returns `Malformed` | n/a | n/a | n/a | Scheduled (separate plan) |
| **D-2** CB7 archive | ❌ stub returns `Malformed` | n/a | n/a | n/a | Scheduled (separate plan) |
| **D-3** ~~Dictionary trigram refresh~~ | ❌ empty job body (intentional) | n/a | n/a | ✅ deferral note added | **Closed M2** (punted to search v1.1) |
| **D-4** ~~Light / amber theme~~ | ✅ both palettes wired (`[data-theme="amber"]` added in `globals.css`) | ✅ toast dropped; description updated | ✅ amber assertion in `theme.test.ts` | ✅ inline comment updated | **Closed M6** |
| **D-5** ~~Admin `/search` page~~ | ✅ search endpoints exist | ✅ placeholder removed | n/a | n/a | **Closed M2** (dropped) |
| **D-6** ~~`fallback_suggestion`~~ | ✅ populated via new `top_on_deck_card` helper | ✅ rendered as suggestion tile in caught-up body | ✅ 2 new server + 3 new vitest tests | ✅ M3 done memo | **Closed M3** |
| **D-7** ~~`prevIssue` keybind~~ | ✅ `GET /issues/{id}/prev-up` + `pick_prev_*` helpers | ✅ `usePrevUp` + `Shift+P` keybind + Reader handler | ✅ 8 new prev_up + 2 new keybind tests | ✅ doc'd in reader-shortcuts.md | **Closed M4** |
| **D-8** ~~Next-up latency histogram~~ | ✅ Drop-on-exit `LatencyTimer` wraps every return path | n/a | n/a | ✅ doc'd in reader-shortcuts.md | **Closed M3** |
| **D-9** ~~OTLP exporter~~ | ✅ env var still read; observability.rs logs clear "not shipped in v1" hint when set | n/a | n/a | ✅ §D-9 decision note | **Resolved** (considered, not chosen) |
| **D-10** ~~`COMIC_ARCHIVE_MAX_*` enforcement~~ | ✅ env vars now read via `Config::archive_limits()`; threaded through ZipLru / scanner / post_scan | n/a | ✅ round-trip test + 37 existing tests still green | ✅ `.env.example` "unenforced" caveat dropped | **Closed M5** |
| **D-11** ~~`progress_records` → Automerge~~ | ✅ table is now permanent | n/a | ✅ unchanged | ✅ spec §9 decision note | **Resolved** (considered, not chosen) |

---

## 6. File-by-file findings

Only files with genuine findings. One line per finding.

### Rust — server

- [crates/archive/src/cbr.rs:25,43](../../crates/archive/src/cbr.rs#L25) — D-1, stub returns `Malformed("CBR support not yet implemented")`
- [crates/archive/src/cb7.rs:19,37](../../crates/archive/src/cb7.rs#L19) — D-2, same shape
- [crates/server/src/api/admin_queue.rs:93](../../crates/server/src/api/admin_queue.rs#L93) — B-1, missing `audit::record`
- [crates/server/src/api/next_up.rs](../../crates/server/src/api/next_up.rs) — D-6 (`fallback_suggestion` always `None`), D-8 (no latency histogram)
- [crates/server/src/api/rails.rs](../../crates/server/src/api/rails.rs) — B-2 server side (`OnDeckCard::CblNext` lacks `saved_view_id`)
- [crates/server/src/api/saved_views.rs:1737,1750](../../crates/server/src/api/saved_views.rs#L1737) — M4 stub comments (intentional, but worth re-verifying)
- [crates/server/src/auth/local.rs:12](../../crates/server/src/auth/local.rs#L12) — S-2, stale comment
- [crates/server/src/jobs/post_scan.rs:817](../../crates/server/src/jobs/post_scan.rs#L817) — D-3, empty TODO stub
- [crates/server/src/library/scanner/reconcile_status.rs:61](../../crates/server/src/library/scanner/reconcile_status.rs#L61) — TODO-by-design (documents existing workaround)
- [crates/server/src/main.rs:17](../../crates/server/src/main.rs#L17) — S-1, `COMIC_LOAD_DOTENV` undocumented
- [crates/server/src/cbl/catalog.rs:80](../../crates/server/src/cbl/catalog.rs#L80) — S-1, `COMIC_GITHUB_TOKEN` undocumented
- [crates/server/src/observability.rs:325-326](../../crates/server/src/observability.rs#L325) — D-9, OTLP deferred (documented)
- [crates/server/src/api/series.rs:1794](../../crates/server/src/api/series.rs#L1794), [auth/csrf.rs:201](../../crates/server/src/auth/csrf.rs#L201), [api/cbl_lists.rs:1022](../../crates/server/src/api/cbl_lists.rs#L1022), [auth/oidc.rs:695](../../crates/server/src/auth/oidc.rs#L695), [auth/cookies.rs:96](../../crates/server/src/auth/cookies.rs#L96), [auth/cookies.rs:127](../../crates/server/src/auth/cookies.rs#L127), [email/mod.rs:149](../../crates/server/src/email/mod.rs#L149) — S-3, six `#[allow(dead_code)]`

### Rust — entity / migration

- [crates/entity/src/progress_record.rs](../../crates/entity/src/progress_record.rs) — D-11, comment updated to reflect "considered, not chosen" decision
- [crates/migration/src/m20260201_000003_progress_placeholder.rs](../../crates/migration/src/m20260201_000003_progress_placeholder.rs) — D-11, header comment updated; file retains `_placeholder` suffix for git-history continuity
- [crates/server/src/api/progress.rs](../../crates/server/src/api/progress.rs) — D-11, module comment updated

### Web — components / app

- [web/components/admin/nav.ts:82](../../web/components/admin/nav.ts#L82) — D-5, `placeholder: true` nav entry
- [web/app/[locale]/(admin)/admin/search/page.tsx](../../web/app/%5Blocale%5D/%28admin%29/admin/search/page.tsx) — D-5, entire page is `<PlaceholderPage />`
- [web/components/admin/PlaceholderPage.tsx](../../web/components/admin/PlaceholderPage.tsx) — single-use placeholder component
- [web/components/settings/ThemePicker.tsx:94-96](../../web/components/settings/ThemePicker.tsx#L94) — D-4, "Coming soon" toast
- [web/app/[locale]/(library)/series/[slug]/issues/[issueSlug]/IssueSettingsMenu.tsx:67](../../web/app/%5Blocale%5D/%28library%29/series/%5Bslug%5D/issues/%5BissueSlug%5D/IssueSettingsMenu.tsx#L67) — S-4, partially stale "Coming soon" comment
- [web/components/library/OnDeckCard.tsx:107,119,137](../../web/components/library/OnDeckCard.tsx#L107) — B-2 web side, `readerUrl(issue)` / `issueUrl(issue)` without `cbl=`

### Web — library-shim type suppressions (intentional, listed for completeness)

- [web/app/[locale]/read/[seriesSlug]/[issueSlug]/marker-selection.ts:135](../../web/app/%5Blocale%5D/read/%5BseriesSlug%5D/%5BissueSlug%5D/marker-selection.ts#L135) — `as any` on `tesseract.js` dynamic import (library-shim)
- [web/components/pages-manager/PagesManager.tsx:516,518](../../web/components/pages-manager/PagesManager.tsx#L516) — `attributes as any` / `listeners as any` for dnd-kit (library-shim)
- [web/components/sidebar-layout/NavigationManager.tsx:537,539](../../web/components/sidebar-layout/NavigationManager.tsx#L537) — same dnd-kit shim

### Documentation

- [.env.example](../../.env.example) — S-1, missing `COMIC_LOAD_DOTENV` + `COMIC_GITHUB_TOKEN`
- [docs/dev/comic-reader-spec.md](comic-reader-spec.md) — D-1 / D-2, CBR/CB7 scope decision not stated

---

## See also

- [docs/dev/phase-status.md](phase-status.md) — milestone-by-milestone status across phases
- [docs/dev/runtime-configuration.md](runtime-configuration.md) — env vs. DB-backed config matrix (target for S-1 docs)
- [CLAUDE.md](../../CLAUDE.md) — project invariants this audit checks against
