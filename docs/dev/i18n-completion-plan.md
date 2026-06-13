# Internationalization (i18n) completion plan

Status: **planned, not started.** This document is the execution-ready plan
for turning Folio's i18n from inert scaffolding into a working feature. It
supersedes the frontend-audit's original A6 recommendation (which was to
*delete* the scaffolding); the decision was reversed on 2026-06-13 — we keep
i18n and finish it.

## TL;DR of the current state (2026-06-13 reconnaissance)

The plumbing is sound and already wired; **no actual translation happens.**

| Layer | State |
| --- | --- |
| next-intl middleware (`web/proxy.ts`, `localePrefix: "never"`) | ✅ working — locale from cookie/`Accept-Language`, URLs stay clean (no `/[locale]/` segment) |
| `createNextIntlPlugin` (`web/next.config.ts`) | ✅ wired |
| `getLocale()` / `getMessages()` + `NextIntlClientProvider` (`web/app/layout.tsx`) | ✅ wired; `<html lang={locale}>` dynamic |
| Locale resolution config (`web/i18n/request.ts`) | ✅ validates against `SUPPORTED_LOCALES`, falls back to `DEFAULT_LOCALE`, dynamically imports `messages/${locale}.json` |
| Per-user `language` preference | ✅ exists end-to-end: DB column, `PATCH /auth/me/preferences { language }`, and the Rust auth path writes `NEXT_LOCALE` cookie on login/refresh from `user.language` |
| Message catalogs | 🟠 only `messages/en.json` — **5 strings, all unused** |
| `useTranslations` / `getTranslations` calls | ❌ **zero** anywhere in the codebase |
| Hardcoded English UI strings | ❌ **~1,400+ across ~236 files** (JSX text, `aria-label`, `placeholder`, `title`, toasts, validation, dialog titles, table headers) |
| `SUPPORTED_LOCALES` | `["en"]` only — declared in **two** places that must stay in lockstep: `web/i18n/request.ts` and `crates/server/src/auth/local.rs` |
| Locale-switcher UI | ❌ none (the `language` preference API is reachable by no component) |
| Date/number formatting | 🟡 raw `.toLocaleString()` in ~14 files; no next-intl formatters, no ICU |
| RTL / `dir` handling | ❌ none |
| i18n tests | ❌ none |

**Implication:** the expensive work is not the plumbing — it's (a) extracting
~1,400 strings into catalogs and replacing them with `t()` calls, (b) a
locale-switcher + negotiation polish, (c) a translation pipeline, and (d) a
lint gate so new code can't reintroduce hardcoded strings. The plumbing being
done means we can ship this incrementally, surface by surface, with English
behaving identically throughout.

## Decisions required before execution

These gate the work and should be answered first (flagged so we don't guess):

1. **Target locales.** Which languages ship first? (Drives catalog count,
   translation cost, and whether M6/RTL is in scope at all.) A pseudolocale
   (`en-XA`) is recommended regardless for QA even if no real second language
   ships immediately.
2. **RTL in scope?** Only needed if a RTL language (ar/he/fa/ur) is targeted.
   If yes, M6 (logical-property sweep) becomes mandatory and non-trivial.
3. **Server-originated strings.** Error-envelope `message`s and validation
   text come from Rust. Localize them client-side keyed by `error.code`
   (leverages the `ApiErrorCode` enum + the `error.details[].field` shape that
   chunk 1.3 added) — **recommended** — or localize server-side via
   `Accept-Language`? Recommendation: client-side by code; the server stays
   English-canonical and the client owns presentation.
4. **Translation pipeline.** Contributor PRs editing JSON, machine-translate +
   human review, or a TMS (Crowdin / Lokalise / Weblate)? Drives M7.
5. **Coverage bar for "done".** Every string, or user-facing surfaces only
   (excluding admin/operator screens that can stay English)? Admin-only is a
   legitimate descoping that roughly halves the string count.

## Conventions (M0 — establish once, enforce forever)

- **Catalog shape:** namespaced by surface, not flat. Top-level namespaces:
  `common` (Save/Cancel/Delete/Loading…), `auth`, `library`, `series`,
  `issue`, `reader`, `markers`, `collections`, `views`, `search`, `settings`,
  `admin`, `errors` (keyed by `ApiErrorCode`), `toasts`. Keys are
  `namespace.area.thing` (e.g. `auth.signIn.title`).
- **next-intl APIs:** `getTranslations()` in server components (keeps strings
  out of the client bundle — see bundle note below), `useTranslations()` in
  client components, `getFormatter()`/`useFormatter()` for dates/numbers/
  relative-time, ICU `{count, plural, ...}` / `{x, select, ...}` for
  count- and gender-dependent strings.
- **Type-safe keys:** augment next-intl's `Messages` type from the en catalog
  in a `global.d.ts` so `t("…")` keys are checked by `tsc` and a typo is a
  build error. (next-intl supports `declare global { interface IntlMessages
  extends (typeof import('./messages/en.json')) {} }`.)
- **Lint gate (the durable guard):** adopt a no-raw-JSX-string rule
  (`eslint-plugin-formatjs` `no-literal-string`, or `eslint-plugin-i18next`)
  in the ESLint flat config, enabled **per-directory** as each surface is
  migrated (override blocks in `web/eslint.config.mjs`), then globally once
  the sweep completes. Without this, the sweep silently rots as new PRs add
  English literals. This mirrors the repo's existing grep-gate pattern.
- **Missing-key behavior:** non-en catalogs fall back to en (next-intl
  default). A CI check asserts every locale catalog has exactly the en key
  set (no missing, no orphan) — see M7.
- **en is canonical:** `messages/en.json` is the source of truth; all keys are
  authored there first, other locales derive from it.

## Bundle-budget interaction (important)

`NextIntlClientProvider` ships whatever `messages` it's given into the client
bundle. Passing the **entire** catalog would add weight to every route — and
the reader route is on a hard budget (currently 195 KB ceiling after the React
Compiler bump; §18.1 target 150 KB). Mitigations, in priority order:

1. **Prefer `getTranslations()` in server components** so most strings never
   reach the client bundle at all.
2. For client components, pass only the **needed namespaces** to the provider
   per route: `<NextIntlClientProvider messages={pick(messages, ['reader',
   'common'])}>`. next-intl supports message subsetting; the reader should
   carry `reader` + `common` only, never `admin`.
3. Keep `messages/en.json` namespaced so subsetting is a cheap object pick.

`pnpm --filter web run check-bundle-size` must stay green on every i18n PR that
touches the reader tree; treat a reader-bundle regression as a blocker.

## Milestones

Each milestone is an independently-mergeable PR (or small PR series), English
behaving identically throughout. Order matters: M0→M2 establish the pattern
before the big sweep.

### M0 — Foundations & conventions
- Decide catalog shape + key naming (above); restructure `messages/en.json`
  into the namespaced skeleton (still English).
- Add the type-safe-keys `global.d.ts` augmentation.
- Add the `pick()` message-subsetting helper for the client provider.
- Add the formatter helpers wrapper (re-export `getFormatter`/`useFormatter`).
- Wire the no-literal-string ESLint rule **off by default** (no enforced dirs
  yet) so it's ready to switch on per-surface.
- No user-visible change.

### M1 — Centralized strings first (cheapest wins)
- Migrate the already-centralized strings: `web/lib/api/toast-strings.ts` →
  `toasts` namespace consumed via `useTranslations`/`getTranslations`.
- Stand up the `errors` namespace keyed by `ApiErrorCode`; add a client helper
  `t(\`errors.${code}\`)` with fallback to the server `message`. This is where
  chunk 1.3's structured `error.code` + `error.details[].field` pay off — the
  field-level messages can localize per field.
- Decision #3 (server-string strategy) is settled here.

### M2 — Pilot vertical slice + pseudolocale
- Fully translate **one complete surface** end-to-end as the reference impl:
  the **auth pages** (sign-in / register / forgot / reset) are the right pilot
  — self-contained, high-traffic, no list/virtualization complexity.
- Add a **pseudolocale** `en-XA` (accented + ~40% length expansion) generated
  from en at build time. This catches untranslated strings (they render in
  plain ASCII) and layout overflow **without needing a real translator**, and
  becomes the QA workhorse for every later surface.
- Turn the lint rule **on** for the auth directory only.
- Establishes: provider subsetting, server-vs-client split, ICU usage,
  type-checked keys, the pseudolocale QA loop, and an i18n test pattern (M8).

### M3 — Locale switcher UI + negotiation polish
- Build the language picker: a select in `/settings/account` (authenticated →
  `PATCH /auth/me/preferences { language }`, which already sets `NEXT_LOCALE`)
  and a lightweight header/footer switcher for unauthenticated visitors (sets
  the `NEXT_LOCALE` cookie client-side + triggers `router.refresh()`).
- Expand `SUPPORTED_LOCALES` in **lockstep** across `web/i18n/request.ts` and
  `crates/server/src/auth/local.rs` (add a guard/test that diffs the two lists
  so they can't drift — the comment-only contract today is fragile).
- Handle the post-switch refresh correctly (cookie change → `router.refresh()`
  so server components re-render under the new locale).

### M4 — Formatting
- Replace raw `.toLocaleString()` (the ~14 files: `CronInput`,
  `LibraryEventsList`, `LibraryList`, `LibraryOverview`, `ScanDashboardClient`,
  `ScanRunsTable`, `HealthIssuesTable`, `UserReadingStats`, `UserProfileForm`,
  `AuditTable`, `RemovedItemsTable`, `UserTable`, `QualityTab`, `UsersTab`,
  plus `BackupStorageCard`'s custom `formatDate`) with next-intl
  `useFormatter().dateTime/number/relativeTime` so formats follow the active
  locale and are testable.
- Convert count strings to ICU plurals (`{count, plural, one {# issue} other
  {# issues}}`).

### M5 — Full extraction sweep (the bulk of the work)
- Surface-by-surface extraction, each its own PR, lint rule switched on per
  directory as it lands. Suggested order (high-traffic → low):
  1. `reader` (mind the bundle budget — server-translate where possible)
  2. `library` grid + `series`/`issue` detail
  3. `markers` / `collections` / `views` / `search`
  4. `settings`
  5. `admin` (largest; descopable per decision #5)
- Each PR: extract literals → `en.json` keys, replace with `t()`, run the
  pseudolocale pass, keep `check-bundle-size` green.
- Track progress with a simple count (strings remaining) in this doc.

### M6 — RTL (only if a RTL locale is in scope — decision #2)
- Sweep physical Tailwind utilities → logical: `ml-/mr-`→`ms-/me-`,
  `pl-/pr-`→`ps-/pe-`, `left-/right-`→`start-/end-`, `text-left/right`→
  `text-start/end`. Tailwind v4 supports logical properties natively.
- Set `dir` on `<html>` from the locale's direction in `layout.tsx`.
- Mirror directional icons (chevrons, back/forward, progress) via
  `rtl:-scale-x-100` or direction-aware components.
- Add a RTL pseudolocale or use the real RTL language for the QA pass.

### M7 — Translation pipeline & catalog CI
- Settle decision #4 (pipeline). Whatever the vehicle, add:
  - A CI check: every `messages/*.json` has exactly the en key set (fail on
    missing or orphan keys). Prevents half-translated catalogs shipping silent
    English fallbacks unnoticed.
  - A pseudolocale build step usable locally + in the Playwright screenshot
    matrix (extend the existing 3-theme matrix with a pseudolocale column).

### M8 — Tests
- A vitest render wrapper that mounts components under
  `NextIntlClientProvider` with a chosen locale (the existing harness already
  mocks `@/lib/api/fetch` + `next/navigation`; add the intl provider).
- Assert: keys resolve (no raw-key leakage), the pilot surface renders
  translated under `en-XA`, formatter output matches per locale, and the
  missing-key guard fires.

## Risks & notes

- **Scale.** ~1,400 strings / ~236 files is the dominant cost. M5 is weeks of
  mechanical work; the lint gate is what makes it finishable (and keeps it
  finished). Descoping admin (decision #5) roughly halves it.
- **Bundle budget.** Covered above — server-translate + per-route subsetting;
  the reader budget is the canary.
- **Lockstep `SUPPORTED_LOCALES`.** Two source-of-truth lists (web + Rust);
  M3 adds a drift guard. Don't add a locale to one without the other.
- **React Compiler.** Now enabled — `useTranslations`/`useFormatter` are hooks
  and memoize cleanly; no special handling.
- **`[locale]` directory.** Routes stay under `web/app/[locale]/` (the
  `localePrefix: "never"` setup keeps URLs clean while the file-system router
  still resolves under the segment). This plan does **not** move them — the
  earlier "flatten to `web/app/`" idea belonged to the deletion path, which is
  cancelled.
- **Effort.** Foundations + pilot + switcher (M0–M3) is the small, high-value
  start (~1–2 weeks). The full sweep (M5) + pipeline (M7) is the long tail and
  scales with locale count and the coverage bar.

## Relationship to the frontend-audit remediation plan

- This replaces chunk 1.0's "delete i18n" with "complete i18n later." Chunk
  1.0's sole already-shipped side effect — the `kind`/`refId` sidebar nav
  refactor in chunk 0.4 — stays; it was independent of i18n.
- Wave 1's remaining chunks (1.1 queryKeys, 1.2 status tokens, 1.4 grid URL
  state, 1.5 WS invalidation, 1.6 cmdk) proceed unchanged, **except** they no
  longer get the "`[locale]` paths disappear" simplification — paths stay under
  `web/app/[locale]/`.
- Execution of this plan is **deferred**; sequence it after the current Wave-1
  foundations land, or whenever prioritized.
