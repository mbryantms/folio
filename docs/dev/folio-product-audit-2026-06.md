# Folio Product Audit — June 2026

**Date:** 2026-06-19
**Scope:** Whole-application audit — every page, workflow, admin surface, dashboard,
log source, and reading journey — across seven personas (first-time user, casual
reader, power user, library admin, self-hoster, mobile user, support/troubleshooting).
**Baseline:** v0.22.0 (`main`), Rust workspace (axum + sea-orm + apalis) + Next.js 16
/ React 19.2 / Tailwind v4 / shadcn-ui web app.

---

## 0. Methodology & confidence note (read this first)

Findings were produced by six parallel code-exploration passes (navigation/IA,
reading experience, library management & metadata, admin/operational surfaces,
backend logging/observability, and UI/a11y/mobile/performance/architecture),
each grounded in file-level reading, then synthesized and de-duplicated here.

**A calibration caveat that matters for triage.** Folio's code is unusually
well-annotated with comments describing *past* bugs and the audits that fixed
them (markers like `C2`, `C3`, `C10`, `E1`…). During synthesis I spot-verified
the highest-severity "this is a bug" claims and found that **several reader
findings were stale** — the exploration pass read a fix's historical comment and
reported the already-remediated problem as current. Confirmed-already-fixed
items are quarantined in **Appendix A** so nobody re-chases them.

**Practical implication:** every finding below labelled *Reported* should be
re-confirmed against current code before implementation; findings labelled
*Verified* were checked during this audit. The bulk of feature-gap and
workflow findings (navigation, library management, admin tooling, logging) are
opinion/gap-oriented rather than "already fixed," and are reliable as written.

Each finding carries **Severity** (Critical / High / Medium / Low),
**Category**, **Complexity** (Small <1d / Medium 1–5d / Large >1wk), and where
useful the **persona** most affected.

**Overall verdict.** Folio is already a strong, architecturally-mature product:
the two-stream observability split, durable `library_events` manifest, WebSocket
live scan progress, codegen-driven API types, `#[handler]`/audit/error-envelope
discipline, virtualized grids, a real design-token system, and a sophisticated
reader (webtoon windowing, RTL, zoom math, marker regions) are genuinely
best-in-class for a self-hosted app. The gap to "commercial-SaaS polish" is
concentrated in **onboarding, cross-surface workflow continuity, operational
troubleshooting depth, and mobile/touch + accessibility finishing** — not in
foundational rework.

---

## 1. Executive Summary

### 1.1 Top 25 highest-impact improvements

| # | Finding | Sev | Cat | Cx |
|---|---------|-----|-----|----|
| 1 | No first-run onboarding (admin: "create your first library"; user: "what is this") | High | UX | M |
| 2 | Empty Home has no actionable next-step CTA (first impression is a blank page) | High | UX | S |
| 3 | Scan batch progress invisible until completion (no cumulative `BatchProgress`) | High | Monitoring | L |
| 4 | No job-queue depth visibility (runaway thumbnail/metadata backlogs undetectable) | High | Monitoring | M |
| 5 | Chunk-rollback drops the culprit file (can't answer "why did this file fail") | High | Scan System | L |
| 6 | No duplicate-detection review/merge UI (detected server-side, invisible to admin) | High | Workflow | L |
| 7 | Ring-buffer logs ephemeral, no export/download during incident response | Medium | Logging | S |
| 8 | Cross-library bulk "select all matching" deferred (only per-series works) | Medium | Workflow | L |
| 9 | Several scanner outcomes are health-only or `debug`-only (encrypted, CBR fail, cover-dl fail, file move) → invisible at default `info` | Medium | Logging | M |
| 10 | Settings have no search/discovery aid (find "marker count toggle" = hunt) | Medium | UX | M |
| 11 | Admin tables overflow on mobile with no responsive collapse / scroll hint | Medium | Mobile | M |
| 12 | No "save this search as a view" (search dead-ends; grid has it) | Medium | Workflow | M |
| 13 | Sort disabled while a search query is active (can't sort results) | Medium | UX | S |
| 14 | Badge renders focus ring on a non-interactive `<div>` (false a11y affordance) | High | A11y | S |
| 15 | `aria-expanded` missing on data-table row expander (state not announced) | Medium | A11y | S |
| 16 | Inline tag creation missing in bulk edit (must pre-create tags elsewhere) | Medium | Workflow | M |
| 17 | Failed-job retry has no attempt count / retry history (transient vs persistent?) | Medium | Monitoring | M |
| 18 | Scan-failure & health rows shown raw; no "top failure reasons" rollup | Medium | Dashboard | S |
| 19 | CBL reading-list context not surfaced in reader chrome ("Issue 3 of 12 in …") | Medium | Reading | S |
| 20 | No metadata batch re-match / retry-failed action (one-at-a-time review) | Medium | Workflow | M |
| 21 | Reading-list (CBL) entries can't be reordered in the UI (export→edit→reimport) | Medium | Workflow | M |
| 22 | Long-running ops (deep-validate, metadata runs) have no live progress surface | Medium | Monitoring | L |
| 23 | Filter state resets on library switch (re-apply every time) | Medium | UX | M |
| 24 | Panel-by-panel / guided view missing (OCR regions exist; not a reading mode) | Medium | Reading | L |
| 25 | 550+ `as` casts + scattered status-color literals erode type-safety/consistency | Low | Architecture | L |

### 1.2 Top 10 quick wins (Small, high ratio)

1. **Empty-Home CTA** — turn "No pinned views yet" text into a primary button that opens Create-Filter-View (#2).
2. **Sort while searching** — drop `disabled={!!q}`; server already accepts `q+sort+order` (#13).
3. **Badge semantics** — remove focus ring from the non-interactive `<div>` (or make it a real button) (#14).
4. **`aria-expanded` on row expanders** in `data-table.tsx` (#15).
5. **"Top failure reasons (7d)"** rollup above the scan-failure list (#18).
6. **CBL context badge** in reader chrome from already-resolved `cbl_list_name/position` (#19).
7. **Promote `info` summaries** for skipped folders / encrypted / CBR-fail counts into the scan-completion event (#9).
8. **"Download logs"** button exporting the current ring buffer to JSON (#7).
9. **Topbar Settings link** — Account is 3 menu levels deep today (nav finding).
10. **Empty-states inside each Views tab** (Filter/Reading-list/Collection) explaining the difference + a create CTA.

### 1.3 Top 10 medium initiatives

1. Post-signup onboarding flow (role-aware) (#1).
2. Settings search + keyword-enriched descriptions (#10).
3. "Save search as view" mapping search state → saved-view conditions (#12).
4. Inline tag creation in bulk-edit multiselect (#16).
5. Failed-job retry history (attempt count + last-N errors) (#17).
6. Metadata batch re-match / retry-selected in Review tab (#20).
7. CBL entry drag-reorder (reuse PageEditor dnd) (#21).
8. Responsive admin tables (column-hide / card-stack < md) (#11).
9. Filter-state persistence across library switches (#23).
10. Quick "add to recent collection" cluster on card hover (recents already tracked).

### 1.4 Top 10 strategic initiatives

1. **Scan-visibility redesign** (§5) — cumulative batch progress, current-file/worker telemetry, actionable post-scan summary, and a troubleshooting drill-down. (#3, #5, #22)
2. **Operational dashboard modernization** (§4) — persona-tiered home, trend sparklines, failure-reason rollups, queue-depth gauges. (#4, #17, #18)
3. **Duplicate-detection & merge workbench** — review, "mark as variant," "merge into primary." (#6)
4. **Cross-library bulk engine** — server `bulk-by-filter` endpoints unlocking "select all matching" everywhere. (#8)
5. **Logging consolidation** (§3) — remove/merge low-value logs; promote operator-relevant ones to events/dashboards; add the missing troubleshooting fields.
6. **Accessibility milestone (Phase 3.5)** — the deferred NVDA/VoiceOver/keyboard pass; finish `inert`/aria-live/contrast/touch-target sweep app-wide.
7. **Mobile-first finishing pass** — tables, dialogs, tab-bar badges, filter access, reader touch ergonomics.
8. **Guided/panel reading mode** — turn the existing OCR region pipeline into a competitive reading feature. (#24)
9. **Design-system hardening** — layout primitives (`PageLayout`/`SectionHeader`), enforced spacing/icon/status-color tokens, fewer `as` casts. (#25)
10. **First-class reading analytics** — promote the activity heatmap/streaks into a polished, trend-aware dashboard with onboarding for empty state.

---

## 2. Findings by category

> Consolidated and de-duplicated across the six passes. Cross-references to the
> summary IDs above in brackets.

### 2.1 UX & Workflow

- **[U1 / #1] No onboarding after first registration.** *High · Medium · Verified (absent).*
  After login the user lands on Home with possibly zero libraries/views and no
  guidance. **Rec:** role-aware post-signup flow — admin → "Set up your first
  library"; user → "Browse / create a filter view"; gate on a `has_onboarded`
  flag. *Impact:* the single biggest first-impression lever; reduces "is it
  broken?" support load.

- **[U2 / #2] Empty Home has no CTA.** *High · Small.* `PageRails` empty state is
  text pointing at Settings. **Rec:** primary button → Create-Filter-View dialog
  + one sentence on what views do.

- **[U3 / #10] Settings not discoverable.** *Medium · Medium.* 3 sections × 3–4
  items, no search; descriptions leak implementation detail. **Rec:** search box
  on the index, keyword-rich descriptions, a "most-changed" quick-links row.

- **[U4 / #13] Sort disabled during search.** *Medium · Small · Verified-ish.*
  `LibraryGridToolbar` gates sort on an active query. **Rec:** allow `q+sort+order`.

- **[U5 / #12] Search dead-ends.** *Medium · Medium.* Grid has "Save as view…";
  search results don't. **Rec:** add it; map search state → saved-view conditions.

- **[U6 / #23] Filter state resets on library switch.** *Medium · Medium.* **Rec:**
  persist per-session or offer "apply to all libraries."

- **[U7] Account settings 3 levels deep.** *Low · Small.* Only reachable via
  footer menu. **Rec:** topbar Settings entry.

- **[U8] Search type-narrowing not advertised.** *Low · Small.* `?category=`
  exists but isn't surfaced. **Rec:** result-count chips ("Series · Issues ·
  Collections").

- **[U9] Bookmarks always default to "All"; no favorites quick-access.** *Low · Small.*
  **Rec:** default-filter preference + a Favorites shortcut.

- **[U10] CBL import lacks progress feedback.** *Low · Small.* **Rec:** progress
  bar + "imported N of M."

- **[U11] Keyboard shortcuts undiscoverable (`?`).** *Low · Small.* **Rec:** a
  one-time "Press ? for shortcuts" hint.

### 2.2 Library management & metadata

- **[L1 / #6] No duplicate review/merge UI.** *High · Large.* Detection exists
  server-side; admins see counts only. **Rec:** Duplicates tab with merge /
  "mark as variant" / "keep primary."

- **[L2 / #8 / #10-dup] Cross-library "select all matching" deferred.** *Medium ·
  Large.* Per-series works; cross-grid intentionally stubbed pending a
  `bulk-by-filter` endpoint. **Rec:** build it; wire `onSelectAllMatching`.

- **[L3 / #16] No inline tag creation in bulk edit.** *Medium · Medium.* Multiselect
  shows existing tags only. **Rec:** "+ Create tag" inline (mirror collections).

- **[L4 / #20] No batch re-match / retry-failed metadata.** *Medium · Medium.*
  Review tab is one-at-a-time. **Rec:** checkboxes + "Retry selected."

- **[L5 / #21] CBL entries not reorderable in UI.** *Medium · Medium.* **Rec:**
  drag-handle (reuse PageEditor dnd) or move-up/down.

- **[L6] No collection "move" (only add).** *Low · Medium.* **Rec:** "Move to
  collection…" (remove-from-source + add-to-target).

- **[L7] Story-arc + credits omitted from bulk edit.** *Low · Small.* Intentional,
  but unexplained. **Rec:** a footer note + read-only "credits vary per issue"
  hint so it doesn't read as broken.

- **[L8] No export of filtered results (CSV/JSON).** *Low · Medium.* **Rec:** grid
  toolbar export for power users / archivists.

- **[L9] Metadata apply diff is field-list only.** *Low · Medium.* **Rec:** a
  "3 fields will change" summary badge + collapsible side-by-side.

- **[L10] No required-field warning when applying metadata.** *Low · Small.* **Rec:**
  flag missing title/year/number before apply.

- **[L11] No bulk "add to page/dashboard."** *Low · Medium.* Pages exist but only
  one-at-a-time add. **Rec:** add to the selection toolbar alongside collection-add.

- **[L12] Quick-add to recent collections missing on cards.** *Low · Small.*
  Recents tracked in localStorage but not surfaced. **Rec:** hover cluster of
  top-3 recent collections.

### 2.3 Reading experience

> **Confidence note:** the reader is the most mature subsystem and produced the
> most stale findings. Items below are the ones that survived verification or are
> genuine competitor gaps. Quarantined false-positives are in Appendix A.

- **[R1 / #19] CBL context absent from reader chrome.** *Medium · Small.* The
  resolver already returns `cbl_list_name`/`position`/`total`; only the
  end-of-issue surfaces use it. **Rec:** show "Issue 3 of 12 in <list>" in chrome
  + a list badge.

- **[R2 / #24] No guided / panel-by-panel reading.** *Medium · Large.* OCR text
  regions exist but aren't a reading mode (Panels/Chunky/KOReader have this).
  **Rec:** a `view-mode: guided` stepping panels by tap/arrow. Strategic.

- **[R3] Incognito mode undiscoverable / not toggleable mid-read.** *Medium ·
  Medium.* `?incognito=1` is read-only at mount, no indicator. **Rec:** chrome
  badge + a settings toggle (peek mode already has a banner — mirror it).

- **[R4] Touch targets below 44px on some reader controls.** *Medium · Small.*
  End-card close (28px), tag remove (12px), copy (24px). **Rec:** pad to 44px hit
  areas (WCAG 2.5.5). *Confirm against current code per §0.*

- **[R5] First-run reader overlay under-explains gestures.** *Medium · Small.* An
  overlay component exists; extend it with swipe/double-tap/long-press + RTL
  diagram. *Confirm scope.*

- **[R6] Reduced-motion not honored on every animation path.** *Medium · Small.*
  Honored for fades/scroll/modal; verify page-turn + zoom + rubber-band. *Confirm.*

- **[R7] Page-fit override management opaque.** *Low · Small.* Per-series override
  can stick silently. **Rec:** always-visible "reset to global," show active level.

- **[R8] Page-strip pre-warms ±12 thumbs even when closed.** *Low · Small.* **Rec:**
  gate warming on strip visibility. *Confirm — may already check a flag.*

### 2.4 Logging, monitoring & operational visibility (high-priority area)

*(Detailed remove/merge/promote/add plan in §3; scan-specific redesign in §5.)*

- **[O1 / #3] Scan-batch progress invisible until done.** *High · Large.* Batches
  exist; no cumulative `BatchProgress` event during member runs. **Rec:** emit
  cumulative totals on each member-run completion + dashboard aggregate bar.

- **[O2 / #4] No job-queue depth visibility.** *High · Medium.* apalis depth never
  surfaced. **Rec:** `folio_apalis_queue_depth{job_kind}` gauge + a "Background
  jobs" dashboard card; warn over threshold.

- **[O3 / #5] Chunk-rollback loses the culprit file.** *High · Large.* On batch
  ingest rollback the failing path/error is dropped, so `files_updated <
  files_seen` is undiagnosable. **Rec:** capture `last_failed_path`+`error_kind`,
  emit an event, log at `warn`.

- **[O4 / #7] Ring-buffer logs ephemeral, no export.** *Medium · Small.* 5k
  in-process buffer lost on restart. **Rec:** "Download logs" (JSON/CSV) +
  document Loki/Cloudwatch shipping for self-hosters.

- **[O5 / #9] Operator-relevant outcomes are `debug`/health-only.** *Medium ·
  Medium.* Encrypted archives, CBR→CBZ failures, cover-download failures, and
  file moves are invisible at default `info`. **Rec:** promote to `info`/`warn`
  with counts in the scan-completion event; emit `File.Moved`,
  `CbrConversionFailed`, `EncryptedArchive`, `CoverDownloadFailed`.

- **[O6 / #17] Failed-job retry lacks history.** *Medium · Medium.* No attempt
  count or last-N errors. **Rec:** store/show `retry_count` + last 3 errors so
  transient vs persistent is obvious.

- **[O7 / #22] Long-running ops have no live surface.** *Medium · Large.*
  Deep-validate / metadata runs can take hours with no progress UI. **Rec:**
  reuse the LiveScanProgress pattern (WS + ETA + phases).

- **[O8] Logs search is plain substring only.** *Medium · Medium.* No
  `field:value` / regex / URL-persisted filters. **Rec:** a light query syntax +
  shareable filter URLs.

- **[O9] Audit feed doesn't group bulk actions.** *Low · High.* 50 rows for one
  bulk-edit. **Rec:** group by request/session id; collapse to a summary row.

- **[O10] Metrics gaps.** *Low–Medium · Small.* No scan-phase-duration histogram,
  no job-retry counter, no files/sec percentile, no prune-activity log. **Rec:**
  add the histograms/counters (cheap, high diagnostic value).

- **[O11] "Never scanned" libraries hidden from stale-scan card.** *Low · Small.*
  **Rec:** surface a "Libraries awaiting first scan" section.

### 2.5 Dashboards (see §4 for full plan)

- **[D1 / #18] Raw failure lists, no rollup.** *Medium · Small.* **Rec:** "Top
  failure reasons (7d)" with counts above the list.
- **[D2] Fixed 7-day windows, not adjustable.** *Low · Small.* **Rec:** 7/30/90/all.
- **[D3] No quota/queue/activity trend sparklines.** *Low–Medium · Medium.* Present-
  state snapshots only. **Rec:** add small trend lines + "approaching limit" ETA.
- **[D4] Thumbnail queue lacks rate/ETA.** *Low–Medium · Medium.* **Rec:** jobs/min
  + ETA + throughput sparkline.
- **[D5] Health-issue payloads parsed client-side (dup'd in two components).**
  *Low · Medium.* **Rec:** server-side `summary` field; shared formatter module.

### 2.6 Accessibility (WCAG 2.2 AA)

- **[A1 / #14] Badge: focus ring on non-interactive `<div>`.** *High · Small.*
  False affordance / SR confusion. **Rec:** remove ring or promote to button.
- **[A2 / #15] `aria-expanded` missing on data-table expander.** *Medium · Small.*
- **[A3] Async selects/combos don't announce loading.** *Medium · Medium.* **Rec:**
  `aria-busy` + polite live region.
- **[A4] `text-[10px]`/`text-xs` + muted-foreground borderline contrast.** *Medium ·
  Low–Medium.* 117 sub-12px instances; table headers ~4.2:1 on dark. **Rec:**
  12px floor; re-check muted-foreground token contrast with axe.
- **[A5] App-wide a11y milestone outstanding (Phase 3.5).** *Medium · Large.* The
  NVDA/VoiceOver/keyboard-only pass is still "not started." **Rec:** schedule it;
  fold in R4/R6 reader items and a focus-management sweep.

### 2.7 Mobile / responsive

- **[M1 / #11] Admin tables overflow with no responsive strategy.** *Medium ·
  Medium.* **Rec:** hide non-essential columns < md or stack to cards; add scroll
  hint.
- **[M2] Dialogs use fixed `max-w-lg`, can clip < 360px.** *Low · Small.* **Rec:**
  `w-[calc(100%-2rem)]` mobile, `sm:max-w-lg` up.
- **[M3] Bottom tab-bar: no count badges; can grow tall in landscape.** *Low ·
  Small.* **Rec:** mirror sidebar marker-count badge; cap height.
- **[M4] Tabs/scroll-areas lack overflow affordance on touch.** *Low · Small/Medium.*
  **Rec:** fade-edge mask / thicker touch scrollbar.
- **[M5] Filter trigger scrolls away on mobile.** *Medium · Medium.* **Rec:**
  sticky filter access (e.g., a bottom-bar filter entry).

### 2.8 Performance & architecture

- **[P1] React Compiler on, but ~118 manual memos remain.** *Low · Large.* Audit
  for redundancy / compiler-skips. **Rec:** profile hot paths; remove redundant
  memos; lint for compiler escapes.
- **[P2 / #25] 550+ `as` casts; scattered status-color literals despite
  `status-tone.ts`.** *Low · Large.* **Rec:** prefer generated types + `satisfies`;
  lint-enforce the status-tone helper.
- **[P3] No shared layout primitives.** *Low · Medium.* Repeated `flex flex-col
  gap-*`. **Rec:** `PageLayout`/`SectionHeader`/`CardGrid`.
- **[P4] Spacing/icon-size conventions scattered.** *Medium · Medium.* **Rec:**
  document tiers; extend density vars beyond opt-in.
- **[P5] Query-key prefix-invalidation is convention-only.** *Low–Medium · Medium.*
  A drift script exists but isn't CI-enforced. **Rec:** enforce in CI.
- **[P6] `Cover` uses native `<img>` lazy, not Next `<Image>`.** *Low · Medium.*
  Acceptable today; Next Image would improve LCP/blur. (Note: this is a
  deliberate tradeoff — the reader bundle budget forbids heavy deps; weigh
  before changing.)
- **[P7] VirtualizedCardGrid doesn't render a next-page skeleton.** *Low · Medium.*
  **Rec:** optional `renderLoading` / trailing skeleton row.

---

## 3. Logging Consolidation Plan

Folio's logging architecture is sound (two-stream split, durable manifest,
deliberate non-overlap). The work is **tuning signal-to-noise** and **closing
troubleshooting gaps**, not restructuring.

**Remove**
- Redundant `debug` "skipped (mtime ≤ last_scanned_at)" per-folder line — replace
  with a single per-scan counter (see *Summarize*).
- Duplicated library-lookup error logs across `api/libraries.rs` call sites →
  collapse to one helper that logs once at a consistent level.

**Merge**
- The two progress-snapshot warnings (JSON encode vs DB persist failure) → one
  `progress_update_failed` log with a `reason` field.
- Health-issue payload formatting duplicated in `FindingsView` + `HealthIssuesTable`
  → one shared formatter (and ideally a server-emitted `summary`).

**Summarize (counter instead of per-item line)**
- Folders skipped unchanged → `series_skipped_unchanged` in `ScanStats` + one
  `info` line at scan end.
- Skipped archive entries / encrypted / malformed → per-scan counts in the
  scan-completion event detail.

**Promote (debug/health-only → operator-visible)**
- Encrypted archive, CBR→CBZ conversion failure, cover-download failure → `info`/
  `warn` **and** a typed health issue with `path` + `error` + a
  `recovery_suggestion`.
- File-move detection (old path gone, same hash elsewhere) → `File.Moved` event
  (`{old_path,new_path,hash}`) + `info` log.
- Library-event prune activity → one `info` line (`rows_deleted`, `retention`).

**Add (missing for troubleshooting)**
- Scan-init context: `trigger` (manual/scheduled/file-watch), `last_scan_age`,
  estimated file count.
- Chunk-rollback culprit: `last_failed_path` + `error_kind` (deadlock vs FK vs
  parse) — the #1 "why did this file fail" enabler.
- Series-identity source chosen (comicinfo / series.json / publisher-hint /
  auto-create) at `debug`.
- File-watch trigger trail (`path`, `event_kind`, debounce) + a
  `file_watch_scans_triggered` counter.
- Job lifecycle for stub handlers (search/dictionary) so "enqueued but nothing
  happened" is explained.

**Metrics to add** (all Small): `folio_scan_phase_duration_seconds{phase}`,
`folio_apalis_queue_depth{job_kind}`, `apalis_job_retries_total{job_kind,result}`,
files/sec histogram.

---

## 4. Dashboard Modernization Plan

**Guiding principle:** persona-tiered surfaces. Casual users want status; power
users want detail; admins want health + trends + drill-down. Today most cards
are present-state snapshots that expose raw rows where a rollup would serve
better, while a few summaries hide the detail needed to act.

**Widgets to add**
- **Background-jobs card** — per-queue depth, in-flight, failed, oldest-pending,
  throughput sparkline (closes O2/D4).
- **Top-failure-reasons rollup** (scans + health) with counts (D1/O11).
- **Trend sparklines** for quota, reading activity, library growth, queue depth
  (D3) + "approaching limit" ETA on quotas.
- **"Libraries awaiting first scan"** section (O11).

**Widgets to combine**
- Health-issue parsing/format logic → one shared, server-fed `summary` (D5).
- Scan-failure list + reasons rollup into a single card with rollup-on-top,
  list-below.

**Widgets to make configurable**
- Time windows (7/30/90/all) on every "recent" card (D2).

**New dashboards / surfaces**
- **Live long-op surface** for deep-validate & metadata runs, reusing
  LiveScanProgress (O7).
- **Duplicates workbench** (L1) — not strictly a dashboard, but the missing
  operational surface that turns a count into an action.

**Visualizations**
- Sparklines for trends (lightweight), small horizontal bar rollups for
  failure-reason distributions, a queue-depth area chart. Reuse the existing
  recharts lazy-loaded stats tabs pattern; keep cards summary-first with
  click-to-drill rather than embedding raw tables.

---

## 5. Library Scan Visibility Redesign (future state)

**Goal:** excellent visibility, minimal noise, fast troubleshooting, clear
progress, actionable summaries — for one library and for "scan all" batches.

**During a scan**
- **Batch-level cumulative progress** (the key gap, O1): "3 / 10 libraries
  complete · 12,418 / ~40,000 files" with a single aggregate bar fed by a new
  cumulative `BatchProgress` event, not by polling member runs.
- **Per-library current operation**: phase, % , ETA, **current file** (name +
  size), and a "stalled?" signal (elapsed-on-current-file) so a scan hung on one
  corrupt file is obvious (admin/ops finding #1).
- **Active-worker view**: how many folder-workers are live and what each is on.
- Cancel **and** a graceful pause/throttle for resource-constrained hosts
  (Medium-value, Large).

**After a scan**
- A **summary card that distinguishes distribution**: not just "10 added" but
  "across N series" + min/median/max, so an anomaly (9 added to one series)
  stands out from the routine.
- Categorized outcomes with counts: added / changed / removed / moved /
  duplicates / malformed / encrypted / unsupported / metadata-changed — each
  click-through to the itemized `library_events`.
- Errors & warnings surfaced as **actionable rows**, each with a recovery hint.
- Performance: per-phase durations + files/sec, with the slow-folder outliers
  called out.

**Troubleshooting (the acceptance test — an admin can answer in < 30s):**
- *Why did this file fail?* → chunk-rollback culprit path + `error_kind` (O3).
- *Why no metadata match?* → the matcher already has rich bucket logic; surface
  the decision (score, gate, blacklist) on the issue, not just "no match."
- *Why skipped?* → `skip_reason` field (size/mtime match, folder fast-path,
  force=false).
- *Why duplicate?* → `File.Moved` / duplicate-content event with both paths +
  hash (O5).
- *Why slow?* → phase-duration histogram + slow-folder outliers.
- *Why did a job fail?* → queue-depth + retry history + last-N errors (O2/O6).

**Noise control:** keep the deliberate non-overlap (malformed/encrypted/dup stay
health-issue-canonical, not double-logged), summarize per-item skips into
counters, and reserve `info` for lifecycle + actionable outcomes.

---

## 6. Product Vision Recommendations (highest leverage)

1. **Make the first five minutes excellent.** Onboarding + empty-state CTAs +
   settings discoverability convert "is this working?" into "this is polished."
   Cheapest path to a SaaS feel.
2. **Own operational troubleshooting.** The scan-visibility redesign + logging
   consolidation + queue/job dashboards would put Folio ahead of Komga/Kavita and
   into Immich/Audiobookshelf territory for self-hoster confidence — the audience
   that evangelizes self-hosted apps.
3. **Workflow continuity across surfaces.** Cross-library bulk actions, "save
   search as view," CBL reorder, duplicate merge, and filter persistence remove
   the dead-ends where power users currently drop to file-system or JSON edits.
4. **Finish accessibility & mobile.** The deferred Phase 3.5 a11y milestone plus a
   mobile finishing pass (tables, dialogs, touch targets, tab-bar badges) close
   the most visible quality gaps and are mostly Small/Medium.
5. **Turn existing depth into features.** OCR regions → guided panel reading;
   activity tracking → a polished analytics dashboard; matcher internals →
   "why this match" explainability. These are differentiators built on assets
   Folio already has.

---

## Appendix A — Verified already-fixed (do NOT re-chase)

Spot-checked during synthesis; current code already handles these. They surfaced
because the codebase comments describe the *historical* bug next to its fix.

- **Finished-issue resume:** `read/.../page.tsx` computes `parkedAtEnd =
  finished && page >= totalPages-1` and resets `initialPage` to 0. ✔
- **Page-load retry UX:** `PageImage.tsx` has silent auto-retry, an error state
  ("Couldn't load this page. Tap to retry."), an 8s "Still loading…" hint, and a
  cache-busted retry `src`. ✔
- **Reader hidden-chrome tabbability:** `ReaderChrome` sets `inert` on the same
  condition as `aria-hidden`; `TapZones` buttons are `tabIndex={-1}` inside an
  `aria-hidden` surface (keyboard pages via the keymap). ✔
- **Double-tap zoom:** implemented for single-page via the center tap-zone
  (`onCenterDoubleTap` + zoom ladder). ✔
- **Progress / session-end writes:** use `fetch(keepalive:true)` to the correct
  `/api/...` paths, read the CSRF token **inside** the write callback, and flush
  on `pagehide` + `visibilitychange→hidden`. The "sendBeacon 404 / stale-CSRF
  403 / tab-hide disarm" findings describe the replaced implementation. ✔
- **Brightness/sepia persistence:** persisted globally via `readerPrefSet`
  (`store.ts` `save()`); not per-tab-only. ✔

## Appendix B — Notable existing strengths (so the roadmap doesn't regress them)

Two-stream observability split + durable `library_events` manifest; WebSocket
live scan/thumbnail progress; deep-linkable filters across admin surfaces;
codegen OpenAPI→TS types with a CI drift gate; `#[handler]` tracing +
`record_admin_action!` audit completeness test + single-site error envelope;
cursor-pagination discipline (no silent truncation); virtualized 10k-item grids;
real HSL design-token system with density variants; mature reader (webtoon
windowing, RTL, zoom/pan math, marker/OCR regions, CBL session context, peek &
incognito modes, customizable keybinds); query-key registry; layered error
boundaries; PWA/safe-area care.
