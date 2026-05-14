# Notifications / Toast Audit

**Date:** 2026-05-14
**Scope:** Every `sonner` toast call site across [web/](../../web/), the central [`useApiMutation`](../../web/lib/api/mutations.ts#L112-L139) helper, the [`<Toaster>`](../../web/components/ui/sonner.tsx) configuration, and the auth / sign-out / reader paths that bypass the mutation helper.
**Verdict:** The base plumbing is sound — `useApiMutation` reliably surfaces a structured error toast on every mutation, and ~25 hooks emit a `successMessage` opt-in. The gaps are in **consistency**, not coverage:
- A persistent split between the central success-toast pattern and ad-hoc `toast.success("…")` calls in dialogs that override otherwise-silent hooks
- ~20 hooks deliberately suppress success feedback (pin / unpin / sidebar toggle / reorder) — the right default for some, the wrong one for others
- Several duplicate strings (`"Want to Read isn't ready yet…"`, `"No changes to save"`, `"Name is required"`) implemented per-call instead of via shared helpers
- The `<Toaster>` mounts with **zero explicit props** (no position, duration, expand, or rich-colors policy) — every visual standard is inherited from sonner defaults, which means the first product decision about toast placement isn't documented anywhere

No raw-error leakage was found. The `apiMutate` wrapper unwraps the server envelope to `err.message`, so what reaches `toast.error()` is the structured `error.message` field from the API, not a raw JSON blob.

## 1. Current implementation

### `<Toaster>` mounting — [web/components/ui/sonner.tsx](../../web/components/ui/sonner.tsx)

Mounted once in the root `app/layout.tsx`. Only theme + class names are configured; **no `position`, `duration`, `expand`, `richColors`, `closeButton`, `gap`, or `offset` are set**. The defaults sonner ships with are: `bottom-right`, 4s duration, single-toast view (no expand), no close button.

```tsx
<Sonner
  theme={theme as ToasterProps["theme"]}
  className="toaster group"
  toastOptions={{ classNames: { /* dark/light surface styling only */ } }}
  {...props}
/>
```

### Central success/error path — [web/lib/api/mutations.ts](../../web/lib/api/mutations.ts#L112-L139)

```tsx
return useMutation<TData | null, Error, TInput>({
  mutationFn: (input) => apiMutate<TData>(build(input)),
  onSuccess: (data, input, ctx) => {
    if (successMessage) {
      const msg = typeof successMessage === "function"
        ? successMessage(data, input) : successMessage;
      toast.success(msg);
    }
    onSuccess?.(data, input, ctx);
  },
  onError: (err, input, ctx) => {
    toast.error(err.message);
    onError?.(err, input, ctx);
  },
  ...rest,
});
```

Default behavior:

- **Errors:** Always `toast.error(err.message)`, where `err.message` is `(await res.json()).error?.message ?? <status>` from [`apiMutate`](../../web/lib/api/mutations.ts#L98-L106) — i.e., the structured `ApiErrorBody.message` per [shared/error.rs](../../crates/shared/src/error.rs).
- **Success:** Silent unless the hook author opts in via `successMessage`.
- **Custom handlers:** `onSuccess` / `onError` callbacks fire **after** the toast — so a caller can append behavior (cache invalidation, redirect, custom toast) but not suppress the default.

### Direct `fetch` / `apiFetch` paths that bypass `useApiMutation`

| Caller | File | Toast on success? | Toast on error? |
|---|---|---|---|
| Sign-out (footer) | [components/shell/UserFooter.tsx:47](../../web/components/shell/UserFooter.tsx#L47) | None | Swallowed (`.catch(() => undefined)`) |
| Sign-out (top-bar) | [components/Chrome.tsx:47](../../web/components/Chrome.tsx#L47) | None | Swallowed |
| Reader per-page progress write | [app/[locale]/read/[seriesSlug]/[issueSlug]/Reader.tsx:501-511](../../web/app/[locale]/read/[seriesSlug]/[issueSlug]/Reader.tsx#L501-L511) | None (intentional) | Swallowed |
| `/auth/me` session check | [components/UserNav.tsx](../../web/components/UserNav.tsx) | None | Swallowed |

## 2. Findings

### F-1 — Central pattern shadowed by ad-hoc `toast.success`

`useUpdateSavedView` is silent on success (no `successMessage`). Two dialog wrappers re-add their own success toast manually:

- [components/saved-views/EditCblMetadataDialog.tsx:143](../../web/components/saved-views/EditCblMetadataDialog.tsx#L143) — `toast.success("View updated")`
- [components/saved-views/EditFilterViewSheet.tsx](../../web/components/saved-views/EditFilterViewSheet.tsx) — `toast.success("View updated")`

This is exactly the shape `successMessage` exists to prevent. Same applies to `useUpdateCblList` (silent) re-wrapped with custom toasts at the call site. **The pattern needs to live on the hook, not the caller.**

### F-2 — Duplicate strings as shibboleths

Same message produced in 3+ places, each independently:

- `"Want to Read isn't ready yet — try again in a moment."`
  - [components/collections/useCoverMenuCollectionActions.tsx](../../web/components/collections/useCoverMenuCollectionActions.tsx)
  - [app/[locale]/(library)/series/[slug]/issues/[issueSlug]/IssueSettingsMenu.tsx:145](../../web/app/[locale]/(library)/series/[slug]/issues/[issueSlug]/IssueSettingsMenu.tsx#L145)
  - [app/[locale]/(library)/series/[slug]/SeriesSettingsMenu.tsx:101](../../web/app/[locale]/(library)/series/[slug]/SeriesSettingsMenu.tsx#L101)

- `"No changes to save"`
  - [components/admin/server/ServerSettingsCards.tsx](../../web/components/admin/server/ServerSettingsCards.tsx) (4 occurrences across 4 admin cards: lines 69, 125, 223, 321)
  - [components/admin/auth/TokensCard.tsx:37](../../web/components/admin/auth/TokensCard.tsx#L37)
  - [components/admin/auth/AuthConfigForm.tsx:82](../../web/components/admin/auth/AuthConfigForm.tsx#L82)
  - [components/admin/email/EmailConfigForm.tsx:93](../../web/components/admin/email/EmailConfigForm.tsx#L93)

- `"Name is required"`
  - [components/collections/AddToCollectionDialog.tsx](../../web/components/collections/AddToCollectionDialog.tsx)
  - [components/collections/CollectionsIndex.tsx](../../web/components/collections/CollectionsIndex.tsx)
  - [components/saved-views/CollectionViewDetail.tsx](../../web/components/saved-views/CollectionViewDetail.tsx)

Each of these should be a single shared helper or constant.

### F-3 — Silent success on user-initiated actions that need feedback

Hooks with no `successMessage` where the user **clicked something** and would benefit from acknowledgement:

| Hook | Surfaces it fires from | Feedback today |
|---|---|---|
| `usePinSavedView` | `/settings/views` switch, `/settings/navigation`, view detail header, rail kebab | Switch flip is the only confirmation. On the catalog, that switch is small + sometimes off-screen. |
| `useSidebarSavedView` | Same as above | Same |
| `useReorderSavedViews` | `/settings/navigation` drag | Position settles into place visually — sufficient |
| `useDeleteSavedView` | `/settings/views` (parent dialog) | AlertDialog confirms; row disappears |
| `useDeleteCblList` | View settings | Row disappears; no toast |
| `useDeleteCollection` | Collections index | Row disappears; no toast |
| `useDeleteMarker` | Marker editor + bookmarks list | **No toast — silent removal** |
| `useRemoveEntryFromCollection` | Cover menu | Item disappears; no toast |
| `useReorderMarkers` | Marker list drag | Position settles — sufficient |
| `useSendTestEmail` | Admin → Email | **No toast — silent success on a button labelled "Send test email"** |
| `useDiscoverOidc` | Admin → Auth | Discovery result lands inline — sufficient |
| `useClearMatchEntry` | Manual-match popover | No toast |

The hooks where the result is *visible without a toast* (reorder, dialog-confirmed delete, drag-and-drop) are correctly silent. The ones where **clicking is the only signal** — pin/sidebar toggles on the catalog, marker delete from a list, "Send test email" — should announce themselves.

### F-4 — Sign-out is silent on every path

[Chrome.tsx:47](../../web/components/Chrome.tsx#L47) and [UserFooter.tsx:47](../../web/components/shell/UserFooter.tsx#L47) both run:

```ts
await fetch("/api/auth/logout", { ... }).catch(() => undefined);
start(() => router.refresh());
```

If the request fails, the user lands on a refreshed authenticated page with no indication that sign-out didn't take. On success, the route change is the only signal — acceptable, but a `toast.success("Signed out")` would close the loop visibly.

### F-5 — Reader per-page progress write swallows errors

[Reader.tsx:501-511](../../web/app/[locale]/read/[seriesSlug]/[issueSlug]/Reader.tsx#L501-L511) fires per-page progress with `.catch(() => { /* best-effort; will retry on next page change */ })`. **This one is intentional and correct** — bursting a toast every page would be ruinous to the reading experience. Documented here so future audits don't flag it.

A failure mode worth surfacing separately: if the **session** is lost mid-read, the user reads page after page with no progress saved and no signal. A possible (out-of-scope) follow-up is a 401 detector in `apiFetch` that promotes the silent retry to a sticky reader-only "Progress not saving" banner.

### F-6 — Auth forms diverge from the rest of the app

Sign-in, register, forgot-password, and reset-password all use **inline banners + per-field validation**, never toasts. Success on these forms is signalled by route navigation or an alternate view (e.g., the "Check your email" state on forgot-password).

This is a defensible choice for forms — keeping errors next to the field that produced them is better than a corner toast — but it should be documented as an intentional standard, not a forgotten exception. (See [§4 standard #6](#4-standardization-recommendations).)

### F-7 — Loading state without a loading toast

The reader's OCR flow ([MarkerEditor.tsx:198-206](../../web/app/[locale]/read/[seriesSlug]/[issueSlug]/MarkerEditor.tsx#L198-L206), [MarkerOverlay.tsx:452](../../web/app/[locale]/read/[seriesSlug]/[issueSlug]/MarkerOverlay.tsx#L452)) uses `toast.loading("Reading text…")` + `toast.dismiss(id)` for an operation that visibly blocks the UI. Good pattern.

`useTriggerScan` and `useImportCblList` are **long-running** but don't surface a loading toast — they show a success "queued" message immediately. That's correct because the work is async on the server and progress comes through `<ScanResultListener>` over WebSocket. Worth documenting so nobody adds a redundant loading toast and ends up with two competing notifications per scan.

### F-8 — Destructive operations without explicit confirmation at the mutation site

Confirmation is currently owned by the **caller**, not the mutation. Verified 2026-05-14 (cleanup M4): **every destructive hook on the audit's list already has an `AlertDialog` at its call site**. No fix required — F-8 was a verification finding, not a gap.

| Hook | Confirmed at |
|---|---|
| `useDeleteLibrary` | [LibraryDangerZone.tsx](../../web/components/admin/library/LibraryDangerZone.tsx) — type-name-to-confirm AlertDialog |
| `useConfirmIssueRemoval` | [RemovedItemsTable.tsx:179-204](../../web/components/admin/library/RemovedItemsTable.tsx#L179-L204) — per-row AlertDialog |
| `useDeleteCollection` | [CollectionViewDetail.tsx:223-249](../../web/components/saved-views/CollectionViewDetail.tsx#L223-L249) |
| `useClearReadingHistory` | [PrivacyControls.tsx:73-106](../../web/components/settings/PrivacyControls.tsx#L73-L106) — "Delete all reading history" |
| `useRevokeAllSessions` | [SessionsCard.tsx:96-122](../../web/components/settings/SessionsCard.tsx#L96-L122) — "Sign out of every session?" |
| `useRevokeAppPassword` | [AppPasswordsCard.tsx:394-416](../../web/components/settings/AppPasswordsCard.tsx#L394-L416) — per-row AlertDialog |
| `useDeleteAllThumbnails` | [ThumbnailsAdmin.tsx:463-493](../../web/components/admin/library/ThumbnailsAdmin.tsx#L463-L493) + [LiveScanProgress.tsx:920-948](../../web/components/admin/library/LiveScanProgress.tsx#L920-L948) |
| `useDeleteSavedView` | [SavedViewsManager.tsx](../../web/components/saved-views/SavedViewsManager.tsx) (added in nav-customization M4) |
| `useDeleteCblList` | CBL view detail page |
| `useForceRecreatePageMap` | [SeriesActions.tsx:130-147](../../web/app/[locale]/(library)/series/[slug]/SeriesActions.tsx#L130-L147) |
| `useClearQueue` | [LiveScanProgress.tsx:950+](../../web/components/admin/library/LiveScanProgress.tsx#L950) — "Clear thumbnail queue?" |

**Edge cases observed during verification, not on the audit list:**

- `useRevokeSession` (single-session, in `SessionsCard`) — **not** wrapped. Fires directly on "Sign out" / "Revoke" click. Defensible: the button label is itself the warning, and revoking another device is mild (it just needs to re-sign-in). Revoking your own current session does sign you out immediately, but the button label says "Sign out" — no surprise. Document as intentional.
- `useDeleteMarker` — has `successMessage: "Removed"` but no confirm at any of its 8 call sites. **Will be addressed in cleanup M3.5** with an Undo affordance instead of confirm (per locked product decision (b)).
- `useDeleteAllReadingHistory` — does not exist as a separate hook; `useClearReadingHistory` is the only delete-history mutation. Audit speculation was incorrect.

### F-9 — Inconsistent custom-action shapes

[IssueSettingsMenu.tsx:156](../../web/app/[locale]/(library)/series/[slug]/issues/[issueSlug]/IssueSettingsMenu.tsx#L156) layers an **undo action** onto a Want-to-Read add toast:

```ts
toast.success("Added \"${issueLabel}\" to Want to Read", { action: { label: "Undo", onClick: ... } });
```

No other "added to X" toast in the app uses the action affordance. Add-to-Collection just shows `"Added to ${collection.name}"` with no undo, even though the operation is equally reversible. Either roll undo out across the family or scope it to the one place undo is uniquely valuable.

### F-10 — `toast.info` used as a no-op acknowledgement

Four admin-form cards emit `toast.info("No changes to save")` when the user submits an unmodified form. This is a **negative result** ("nothing happened"), not an info. Consider:

- Disabling the submit button while `isDirty=false` so the path can't be reached (preferred — silently correct UX)
- Or downgrade to `toast.message(…)` (sonner's neutral variant) and accept it as a hint

`toast.info` for "this is broken / coming soon" is fine. Using it as a "you didn't do anything" is conflating two states.

### F-11 — `toast.message` vs `toast.success` are mixed for the same intent

- [Reader.tsx:439](../../web/app/[locale]/read/[seriesSlug]/[issueSlug]/Reader.tsx#L439) — `toast.message(nowHidden ? "Markers hidden" : "Markers shown")` for a toggle
- [Reader.tsx:163-223](../../web/app/[locale]/read/[seriesSlug]/[issueSlug]/Reader.tsx#L163-L223) — `toast.success("Bookmarked page X")`, `toast.success("Unstarred page X")` for related keybind toggles

The keybind toggle action is a state flip — same shape as "Markers shown/hidden" but rendered differently. Pick one: state-flip events all use `toast.message`, completion events use `toast.success`.

## 3. Notification consistency checklist

A meaningful user action surfaces feedback when **all four** of these are true:

1. **It was user-initiated** (button click, keybind, drag-drop). Not background sync, not autosave, not progress write.
2. **The result is not visible without scrolling, opening a menu, or refreshing.** If a row appears/disappears in view, the visual is enough.
3. **The operation could plausibly fail** (network, permission, validation). Pure local UI state flips don't qualify.
4. **The user can act on a failure.** "Permission denied" is actionable; "internal server error" routes to retry-or-report.

Apply this rubric to every mutation:

| State | What surfaces it | Default | Exceptions |
|---|---|---|---|
| Success of a state-changing action | `toast.success(<verb> <object>)` | Always when the result isn't visible inline | Drag-reorder (visible), inline form submit that navigates away (route is the signal) |
| Failure | `toast.error(err.message)` | Always (already automatic via `useApiMutation`) | Best-effort retries (progress writes) — comment + suppress |
| Long-running (>200ms) | `toast.loading("…")` + `toast.dismiss(id)` | Only when the UI doesn't otherwise indicate work | Async-queued operations that get a "queued" success and surface completion via a separate channel (scans, thumbnails) |
| Destructive action | `AlertDialog` confirm → mutation → `toast.success` | Always for irreversible (delete, revoke, clear-history) | Easily-recreatable (marker, bookmark) — flag for product |
| Validation error before request | `toast.error("<field> is required")` (or inline) | Inline preferred; toast only when no field has focus | — |
| Form submit with no changes | Disable submit while `!isDirty` | Always preferred over a "nothing happened" toast | — |

## 4. Standardization recommendations

1. **Pin the `<Toaster>` config.** Drop the defaults; set the policy:
   ```tsx
   <Sonner
     theme={…}
     position="bottom-right"        // current de-facto default — make it explicit
     duration={4000}                // sonner default; document it
     expand={false}                 // keep stacking to one at a time
     gap={8}
     visibleToasts={3}              // bound the queue so a burst can't bury the page
     closeButton                    // affords manual dismissal for any sticky toast
     toastOptions={{ classNames: { … } }}
   />
   ```
   Result: every product decision about placement is in one file.

2. **`successMessage` is the only source of success toasts.** Migrate every `toast.success("View updated")` from dialog callers onto its mutation hook (`useUpdateSavedView`, `useUpdateCblList`, etc.). Audit policy: a grep for `toast.success(` in any file under `components/` should be empty *unless* the toast is action-augmented (undo button) or the toast text depends on inputs the hook doesn't have.

3. **Shared strings.** Extract to `web/lib/api/toast-strings.ts`:
   ```ts
   export const TOAST = {
     WTR_NOT_READY: "Want to Read isn't ready yet — try again in a moment.",
     NO_CHANGES: "No changes to save",
     NAME_REQUIRED: "Name is required",
     // …
   } as const;
   ```
   And/or extract `toast.info("No changes to save")` into a `useSaveOnDirty` form helper that disables the submit when `!isDirty` (preferred — UX cleanup, not a string move).

4. **Silent-success policy.** Document which actions are intentionally silent and **why**, on the hook itself. Suggested defaults:
   - **Silent:** drag-reorder, AlertDialog-confirmed delete, OAuth/session reads, autosave, progress writes, sidebar toggle (the actual sidebar redraw is the signal)
   - **Toasted:** pin/unpin (the rail change happens off-screen), delete from a list without confirm (marker, collection entry), "send test email" (one-shot async with no visible outcome), revoke session, generate app password
   Recommendation: switch `useDeleteMarker`, `useSendTestEmail`, `useRevokeAllSessions`, `usePinSavedView`, `useSidebarSavedView` from silent to `successMessage`-bearing.

5. **Destructive-action confirmation matrix.** Every irreversible mutation gets a confirm at the call site. Add `AlertDialog` wrappers to the current unconfirmed callers in [§F-8](#f-8--destructive-operations-without-explicit-confirmation-at-the-mutation-site).

6. **Auth-form standard.** Document the divergence: forms with field-level validation (sign-in, register, forgot, reset, change-password) use **inline banners** + `<FormMessage>`; everywhere else uses toasts. Add a one-line comment to each form file pointing back to this audit so the standard is discoverable.

7. **Sign-out feedback.** Add a `toast.success("Signed out")` after both `fetch("/api/auth/logout")` calls. Keep the error path silent — if the network is unhealthy, the user finds out via the next request.

8. **`toast.info` only for product-state messages.** Reserve `toast.info` for "feature flagged / coming soon / discovery results"; downgrade "no changes" cases to a disabled-submit pattern or `toast.message`.

9. **`toast.message` vs `toast.success` decision.** Adopt:
   - `toast.message` for **toggles** (markers shown/hidden, sound on/off — symmetric flips)
   - `toast.success` for **completion** of an action with an "after" state (pinned, saved, deleted, queued)
   - `toast.error` for failure
   - `toast.info` for app-state notices (feature unavailable, plan limits)
   - `toast.loading` only when the UI cannot otherwise show progress

10. **No raw error leakage — keep it that way.** [`apiMutate`](../../web/lib/api/mutations.ts#L98-L106) unwraps `error.message` from the server envelope before throwing. Don't bypass that path: any new `fetch()` call should either route through `apiFetch` + `apiMutate` or hand-roll the same `.error?.message` extraction.

## 5. Missing-coverage matrix

Action that should produce feedback **but doesn't today**. `Verify` = confirm in code before changing (e.g., a parent dialog may already wrap it).

| Action | Hook / file | Today | Recommended |
|---|---|---|---|
| Pin / unpin from catalog switch | `usePinSavedView` | Switch flip only | `toast.success("Pinned to home" / "Removed from home")` |
| Show in / hide from sidebar | `useSidebarSavedView` | Switch flip only | `toast.success("Added to sidebar" / "Removed from sidebar")` |
| Delete a marker from the bookmarks list | `useDeleteMarker` | Row disappears, no toast | `toast.success("Marker deleted")` + AlertDialog (verify desire) |
| Send test email | `useSendTestEmail` | Silent | `toast.success("Test email sent")` / `toast.error(<server error>)` |
| Revoke all sessions | `useRevokeAllSessions` | (verify) | AlertDialog confirm + `toast.success("All sessions revoked")` |
| Sign out | direct `fetch` in `Chrome.tsx`, `UserFooter.tsx` | Silent on both paths | `toast.success("Signed out")` after success |
| Delete library | `useDeleteLibrary` | "Deleted library" success exists | Verify AlertDialog confirm wraps it |
| Permanent remove issue | `usePermanentlyDeleteIssue` | "Removal confirmed" success | Verify dialog exists; if not, add |
| Clear reading history | `useClearReadingHistory` | "Reading history cleared" success | Verify AlertDialog confirm wraps it |
| Delete all reading history | `useDeleteAllReadingHistory` | "All reading history cleared" success | Verify AlertDialog confirm wraps it |
| Reset sidebar layout | `useUpdateSidebarLayout({ entries: [] })` | `toast.success("Sidebar reset to defaults")` (in NavigationManager) | Already correct |
| Add to Want to Read | per-call `toast.success(…)` | Undo affordance only on issue path | Either propagate `action: { Undo }` to `useAddEntryToCollection` callers, or drop the action — pick one |
| Add to Collection | per-call `toast.success(…)` | No undo affordance | Mirror Want-to-Read pattern (with action) once decision is made |
| Manual-match cleared | `useClearMatchEntry` | Silent (comment claims toast comes from useApiMutation, but no successMessage is set) | `toast.success("Match cleared")` |

## 6. Patterns to remove

- Custom `toast.success("View updated")` overrides at [EditCblMetadataDialog.tsx:143](../../web/components/saved-views/EditCblMetadataDialog.tsx#L143) and [EditFilterViewSheet.tsx](../../web/components/saved-views/EditFilterViewSheet.tsx) → fold into `useUpdateSavedView` and `useUpdateCblList` `successMessage`.
- Three duplicate `"Want to Read isn't ready yet…"` strings → extract.
- Four duplicate `"No changes to save"` info toasts → replace with `disabled={!isDirty}` on the submit button.
- Three duplicate `"Name is required"` validation toasts → fold into react-hook-form `register` validators or a shared helper.

## 7. Phase plan

Suggested rollout order if this is implemented as a follow-up:

1. **Lock `<Toaster>` config** ([§4 #1](#4-standardization-recommendations)) — single file, no risk.
2. **Hook migrations** ([§F-1](#f-1--central-pattern-shadowed-by-ad-hoc-toastsuccess)) — move custom `toast.success` calls back to `successMessage`. Per hook: trivial 1-line change + delete the caller line. Net file count down.
3. **String extraction** ([§F-2](#f-2--duplicate-strings-as-shibboleths)) — `toast-strings.ts`. Mechanical.
4. **Silent-success flips** ([§4 #4](#4-standardization-recommendations)) — add `successMessage` to the five hooks listed.
5. **Destructive-action confirms** ([§F-8](#f-8--destructive-operations-without-explicit-confirmation-at-the-mutation-site)) — audit each parent, add AlertDialog wrappers where missing. Largest cleanup.
6. **Sign-out toast** ([§F-4](#f-4--sign-out-is-silent-on-every-path)) — two-line change in two files.
7. **`toast.info` cleanup** ([§F-10](#f-10--toastinfo-used-as-a-no-op-acknowledgement)) — convert "no changes" cases to disabled submit.
8. **Document the auth-form divergence** ([§F-6](#f-6--auth-forms-diverge-from-the-rest-of-the-app)) — one-line comments in each auth form pointing to this doc.

Each phase ships independently; the doc itself is the spec.
